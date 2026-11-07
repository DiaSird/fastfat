//! Volume devices: paths, disk extents, locking and discovery.

use std::fs::{File, OpenOptions};
use std::io;
use std::os::windows::fs::OpenOptionsExt;
use std::ptr;
use std::thread;
use std::time::Duration;

use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
use windows_sys::Win32::Storage::FileSystem::{
    FILE_SHARE_READ, FILE_SHARE_WRITE, FindFirstVolumeW, FindNextVolumeW, FindVolumeClose,
    GetVolumeInformationW, IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS,
};
use windows_sys::Win32::System::Ioctl::{
    DISK_EXTENT, FSCTL_ALLOW_EXTENDED_DASD_IO, FSCTL_DISMOUNT_VOLUME, FSCTL_LOCK_VOLUME,
    FSCTL_UNLOCK_VOLUME, IOCTL_DISK_GET_PARTITION_INFO_EX, PARTITION_INFORMATION_EX,
    PARTITION_STYLE_GPT, PARTITION_STYLE_MBR,
};

use super::PartitionStyle;
use super::ioctl::{context, from_wide, ioctl_none, ioctl_query, wide};

/// Maximum number of disk extents read for one volume.
const MAX_EXTENTS: usize = 16;

/// How often [`wait_for_volume_root`] looks for the volume.
const POLL_INTERVAL: Duration = Duration::from_millis(500);

/// How many times [`wait_for_volume_root`] looks for the volume (30 s).
const POLL_COUNT: u32 = 60;

/// Returns the letter of the Windows system drive.
pub(super) fn system_letter() -> char {
    std::env::var("SystemDrive")
        .ok()
        .and_then(|s| s.chars().next())
        .map_or('C', |c| c.to_ascii_uppercase())
}

/// Returns the device path `\\.\X:` of a drive letter.
pub(super) fn volume_path(letter: char) -> String {
    format!(r"\\.\{letter}:")
}

/// Returns the number of the disk holding the Windows system drive.
pub(super) fn system_disk() -> io::Result<u32> {
    single_disk(&open_query(&volume_path(system_letter()))?)
}

/// Opens a volume or disk without access rights, for queries only.
///
/// Such handles do not need administrator rights.
pub(super) fn open_query(path: &str) -> io::Result<File> {
    OpenOptions::new()
        .access_mode(0)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .open(path)
}

/// `VOLUME_DISK_EXTENTS` with room for [`MAX_EXTENTS`] extents.
#[repr(C)]
#[derive(Default)]
struct DiskExtents {
    count: u32,
    extents: [DISK_EXTENT; MAX_EXTENTS],
}

impl DiskExtents {
    /// Queries the extents of a volume.
    fn query(file: &File) -> io::Result<Self> {
        ioctl_query(file, IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS)
    }

    /// Returns the extents reported by the driver.
    fn as_slice(&self) -> &[DISK_EXTENT] {
        let count = usize::try_from(self.count).map_or(MAX_EXTENTS, |n| n.min(MAX_EXTENTS));
        &self.extents[..count]
    }
}

/// Returns the disk number of a volume that lies on exactly one disk.
pub(super) fn single_disk(file: &File) -> io::Result<u32> {
    match DiskExtents::query(file)?.as_slice() {
        [extent] => Ok(extent.DiskNumber),
        _ => Err(io::Error::other("the volume spans several disks")),
    }
}

/// Returns the partition table style of the disk holding a volume.
pub(super) fn partition_style(file: &File) -> io::Result<PartitionStyle> {
    let info: PARTITION_INFORMATION_EX = ioctl_query(file, IOCTL_DISK_GET_PARTITION_INFO_EX)?;
    Ok(style_from_raw(info.PartitionStyle))
}

/// Converts a Win32 `PARTITION_STYLE` value.
pub(super) const fn style_from_raw(style: i32) -> PartitionStyle {
    match style {
        PARTITION_STYLE_MBR => PartitionStyle::Mbr,
        PARTITION_STYLE_GPT => PartitionStyle::Gpt,
        _ => PartitionStyle::Raw,
    }
}

/// Returns the device paths (`\\?\Volume{GUID}`) of all volumes.
fn volume_paths() -> io::Result<Vec<String>> {
    let mut name = [0_u16; 64];
    // SAFETY: `name` is writable for the length passed.
    let handle = unsafe { FindFirstVolumeW(name.as_mut_ptr(), name.len() as u32) };
    if handle == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    let mut paths = Vec::new();
    loop {
        // Volume names end with a backslash, which must be removed to open
        // the volume device rather than its root directory.
        let mut path = from_wide(&name);
        path.truncate(path.trim_end_matches('\\').len());
        paths.push(path);
        // SAFETY: `handle` is a valid find handle and `name` is writable.
        if unsafe { FindNextVolumeW(handle, name.as_mut_ptr(), name.len() as u32) } == 0 {
            break;
        }
    }
    // SAFETY: `handle` is a valid find handle and is closed exactly once.
    unsafe { FindVolumeClose(handle) };
    Ok(paths)
}

/// Returns the device paths of the volumes with an extent on `disk` that
/// satisfies `filter` (called with the byte offset of the extent).
fn volumes_on_disk(disk: u32, mut filter: impl FnMut(u64) -> bool) -> io::Result<Vec<String>> {
    let mut found = volume_paths()?;
    found.retain(|path| {
        let Ok(extents) = open_query(path).and_then(|file| DiskExtents::query(&file)) else {
            return false;
        };
        extents
            .as_slice()
            .iter()
            .any(|e| e.DiskNumber == disk && u64::try_from(e.StartingOffset).is_ok_and(&mut filter))
    });
    Ok(found)
}

/// Locks and dismounts every volume that has an extent on `disk`.
pub(super) fn lock_disk_volumes(disk: u32) -> io::Result<Vec<Volume>> {
    volumes_on_disk(disk, |_| true)?
        .iter()
        .map(|path| Volume::open(path))
        .collect()
}

/// Returns the root directory (`\\?\Volume{GUID}\`) of the mounted volume
/// that starts at byte `offset` of `disk`, if there is one.
pub(super) fn find_volume_root(disk: u32, offset: u64) -> io::Result<Option<String>> {
    let mut paths = volumes_on_disk(disk, |start| start == offset)?;
    let Some(mut root) = paths.pop() else {
        return Ok(None);
    };
    root.push('\\');
    Ok(is_mounted(&root).then_some(root))
}

/// Waits until Windows has mounted the volume at byte `offset` of `disk`
/// and returns its root directory.
///
/// # Errors
///
/// Returns an error when the volume does not appear within 30 seconds.
pub(super) fn wait_for_volume_root(disk: u32, offset: u64) -> io::Result<String> {
    for _ in 0..POLL_COUNT {
        if let Some(root) = find_volume_root(disk, offset)? {
            return Ok(root);
        }
        thread::sleep(POLL_INTERVAL);
    }
    Err(io::Error::new(
        io::ErrorKind::TimedOut,
        "the new volume was not mounted by Windows",
    ))
}

/// Returns whether a file system is mounted at the root directory `root`.
fn is_mounted(root: &str) -> bool {
    let root = wide(root);
    // SAFETY: `root` is NUL terminated; all output buffers are null with a
    // length of 0, which the API allows.
    unsafe {
        GetVolumeInformationW(
            root.as_ptr(),
            ptr::null_mut(),
            0,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            0,
        ) != 0
    }
}

/// A locked and dismounted volume opened for raw access.
///
/// The lock is released when the value is dropped.
pub(super) struct Volume {
    /// Handle with read and write access.
    pub(super) file: File,
}

impl Volume {
    /// Opens the volume device at `path`, then locks and dismounts it.
    pub(super) fn open(path: &str) -> io::Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
            .open(path)
            .map_err(|e| context(e, &format!("cannot open {path} (run as administrator)")))?;

        ioctl_none(&file, FSCTL_LOCK_VOLUME).map_err(|e| {
            context(
                e,
                &format!("cannot lock {path} (close programs and windows using it)"),
            )
        })?;
        ioctl_none(&file, FSCTL_DISMOUNT_VOLUME)
            .map_err(|e| context(e, &format!("cannot dismount {path}")))?;
        // Allows writing past the end of the old file system. Not all
        // drivers support it, and it is not needed once dismounted.
        let _ = ioctl_none(&file, FSCTL_ALLOW_EXTENDED_DASD_IO);
        Ok(Self { file })
    }
}

impl Drop for Volume {
    fn drop(&mut self) {
        // Closing the handle also releases the lock; unlocking explicitly
        // lets Windows remount the volume right away.
        let _ = ioctl_none(&self.file, FSCTL_UNLOCK_VOLUME);
    }
}
