//! Enumeration of volumes with a drive letter.

use std::fmt::Write as _;
use std::ptr;

use windows_sys::Win32::Storage::FileSystem::{
    GetDiskFreeSpaceExW, GetDriveTypeW, GetLogicalDrives, GetVolumeInformationW,
};
use windows_sys::Win32::System::WindowsProgramming::{DRIVE_FIXED, DRIVE_REMOVABLE};

use super::PartitionStyle;
use super::ioctl::{from_wide, wide};
use super::volume::{open_query, partition_style, single_disk, system_letter, volume_path};
use fastfat_units::write_size;

/// Kind of drive as reported by `GetDriveTypeW`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DriveKind {
    /// Removable media such as USB flash drives and memory cards.
    Removable,
    /// Fixed disks such as internal or external HDDs and SSDs.
    Fixed,
}

/// A mounted volume that can be formatted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DriveInfo {
    /// Drive letter, e.g. `'E'`.
    pub letter: char,
    /// Kind of drive.
    pub kind: DriveKind,
    /// Current volume label, if the volume is readable.
    pub label: String,
    /// Current file system name, if the volume is readable.
    pub file_system: String,
    /// Total size in bytes, if the volume is readable.
    pub size: Option<u64>,
    /// Number of the physical disk (`\\.\PhysicalDriveN`) holding the volume.
    pub disk: Option<u32>,
    /// Partition table style of that disk.
    pub partition_style: Option<PartitionStyle>,
    /// One-line description for display in a list, built once at
    /// enumeration time so that the GUI can borrow it every frame.
    pub description: String,
}

/// Returns the letters of the drives present, from `A` to `Z`.
pub(super) fn present_letters() -> impl Iterator<Item = char> {
    // SAFETY: `GetLogicalDrives` takes no arguments.
    let mask = unsafe { GetLogicalDrives() };
    (b'A'..=b'Z')
        .enumerate()
        .filter(move |(bit, _)| mask & (1 << bit) != 0)
        .map(|(_, letter)| char::from(letter))
}

/// Lists volumes with a drive letter that may be formatted.
///
/// The system drive is never listed. Fixed drives are only included when
/// `include_fixed` is `true`.
#[must_use]
pub fn list_drives(include_fixed: bool) -> Vec<DriveInfo> {
    let system = system_letter();
    present_letters()
        .filter(|&letter| letter != system)
        .filter_map(|letter| query_drive(letter, include_fixed))
        .collect()
}

/// Collects information about a single drive letter.
fn query_drive(letter: char, include_fixed: bool) -> Option<DriveInfo> {
    let root = wide(&format!("{letter}:\\"));

    // SAFETY: `root` is a NUL terminated UTF-16 string.
    let kind = match unsafe { GetDriveTypeW(root.as_ptr()) } {
        DRIVE_REMOVABLE => DriveKind::Removable,
        DRIVE_FIXED if include_fixed => DriveKind::Fixed,
        _ => return None,
    };

    let mut label = [0_u16; 261];
    let mut file_system = [0_u16; 261];
    // SAFETY: the buffers are valid for the lengths passed; optional
    // out-parameters are null.
    let has_volume = unsafe {
        GetVolumeInformationW(
            root.as_ptr(),
            label.as_mut_ptr(),
            label.len() as u32,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            file_system.as_mut_ptr(),
            file_system.len() as u32,
        )
    } != 0;

    let mut total = 0_u64;
    // SAFETY: `total` is a valid out-pointer; other out-parameters are null.
    let has_size = unsafe {
        GetDiskFreeSpaceExW(
            root.as_ptr(),
            ptr::null_mut(),
            &raw mut total,
            ptr::null_mut(),
        )
    } != 0;

    let (label, file_system) = if has_volume {
        (from_wide(&label), from_wide(&file_system))
    } else {
        (String::new(), String::new())
    };

    let query = open_query(&volume_path(letter)).ok();
    let mut info = DriveInfo {
        letter,
        kind,
        label,
        file_system,
        size: has_size.then_some(total),
        disk: query.as_ref().and_then(|file| single_disk(file).ok()),
        partition_style: query.as_ref().and_then(|file| partition_style(file).ok()),
        description: String::new(),
    };
    info.description = describe(&info);
    Some(info)
}

/// Builds the one-line description of a drive.
fn describe(info: &DriveInfo) -> String {
    let kind = match info.kind {
        DriveKind::Removable => "Removable",
        DriveKind::Fixed => "Fixed",
    };
    let file_system = if info.file_system.is_empty() {
        "RAW"
    } else {
        &info.file_system
    };
    let mut text = format!("{}:", info.letter);
    if !info.label.is_empty() {
        let _ = write!(text, " \"{}\"", info.label);
    }
    let _ = write!(text, "  [{kind}, ");
    if let Some(disk) = info.disk {
        let _ = write!(text, "Disk {disk} ");
    }
    if let Some(style) = info.partition_style {
        let _ = write!(text, "{}, ", style.name());
    }
    let _ = write!(text, "{file_system}, ");
    match info.size {
        Some(size) => write_size(&mut text, size),
        None => text.push_str("unknown size"),
    }
    text.push(']');
    text
}
