//! Cloning a whole disk: entry point, verbatim copy and shared helpers.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::ops::ControlFlow;

use windows_sys::Win32::System::Ioctl::IOCTL_DISK_UPDATE_PROPERTIES;

use super::disks::{disk_size, open_disk};
use super::ioctl::ioctl_none;
use super::rebuild::clone_rebuilt;
use super::volume::{Volume, lock_disk_volumes, system_disk};
use fastfat_clone as clone;
use fastfat_fat32::{self as fat32, FormatError, Progress};
use fastfat_units::format_size;

/// Bytes wiped at the end of a larger destination (stale backup GPT).
const TAIL: u64 = 1024 * 1024;

/// How the destination is laid out.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum CloneMode {
    /// Copy every sector; space beyond the size of the source stays
    /// unallocated.
    #[default]
    Verbatim,
    /// Copy the partitions behind the FAT32 partition verbatim to the end of
    /// the destination and grow the FAT32 partition into the space in
    /// between (see [`fastfat_mbr::plan_expansion`]).
    ExpandFat32,
    /// Lay the destination out for `wafel_usb_partition`: the FAT32 partition
    /// first, filling the disk except for an empty Wii U storage partition of
    /// at least `storage_bytes` bytes at the end (see
    /// [`fastfat_mbr::plan_fresh_storage`]). The console formats that
    /// partition itself; other partitions of the source are not copied.
    WiiUFreshStorage {
        /// Minimum size of the storage partition; 0 for none.
        storage_bytes: u64,
    },
}

/// Options for [`clone_disk`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CloneOptions {
    /// Give an MBR clone a fresh disk signature, so that Windows does not take
    /// it offline because of a signature collision.
    pub new_signature: bool,
    /// Layout of the destination.
    pub mode: CloneMode,
}

/// The two disks of a clone, opened and checked.
pub(super) struct Pair {
    pub(super) source: File,
    pub(super) destination: File,
    pub(super) source_len: u64,
    pub(super) destination_len: u64,
    pub(super) sector: u32,
}

/// Clones the whole disk `source` onto `destination`.
///
/// With [`CloneMode::Verbatim`] everything is copied sector by sector: the
/// partition table and every partition, including hidden ones without a
/// drive letter. The other modes rebuild the partition table, create a new
/// FAT32 file system and copy its files from the mounted source volume.
///
/// Both disks must use the same logical sector size; for a verbatim copy the
/// destination must also be at least as large as the source. This requires
/// administrator rights.
///
/// Returns the number of bytes copied.
///
/// # Errors
///
/// Returns [`FormatError::Io`] when the disks are the same, the destination
/// holds the Windows system drive, is too small or uses another sector size,
/// a volume cannot be locked, or a read or write fails,
/// [`FormatError::UnsupportedPartitionTable`] when the source table cannot be
/// rebuilt, and [`FormatError::Cancelled`] when the callback cancelled the
/// copy (the destination is then incomplete).
pub fn clone_disk<F>(
    source: u32,
    destination: u32,
    options: CloneOptions,
    progress: F,
) -> Result<u64, FormatError>
where
    F: FnMut(Progress) -> ControlFlow<()>,
{
    let verbatim = options.mode == CloneMode::Verbatim;
    let pair = open_pair(source, destination, verbatim)?;
    if verbatim {
        clone_verbatim(pair, source, destination, options.new_signature, progress)
    } else {
        clone_rebuilt(pair, source, destination, options, progress)
    }
}

/// Opens both disks and checks them; with `need_room` the destination must
/// be at least as large as the source.
fn open_pair(source: u32, destination: u32, need_room: bool) -> Result<Pair, FormatError> {
    if source == destination {
        return Err(io::Error::other("the source and the destination are the same disk").into());
    }
    if system_disk().is_ok_and(|system| system == destination) {
        return Err(io::Error::other("the destination holds the Windows system drive").into());
    }
    let source_file = open_disk(source, false)?;
    let destination_file = open_disk(destination, true)?;
    let (source_len, source_sector) = disk_size(&source_file)?;
    let (destination_len, destination_sector) = disk_size(&destination_file)?;
    if source_sector != destination_sector {
        return Err(io::Error::other(format!(
            "sector sizes differ (source {source_sector} bytes, destination {destination_sector} bytes)"
        ))
        .into());
    }
    if need_room && destination_len < source_len {
        return Err(io::Error::other(format!(
            "the destination is smaller than the source ({} < {})",
            format_size(destination_len),
            format_size(source_len)
        ))
        .into());
    }
    Ok(Pair {
        source: source_file,
        destination: destination_file,
        source_len,
        destination_len,
        sector: source_sector,
    })
}

/// Copies the source disk verbatim.
fn clone_verbatim<F>(
    mut pair: Pair,
    source: u32,
    destination: u32,
    new_signature: bool,
    progress: F,
) -> Result<u64, FormatError>
where
    F: FnMut(Progress) -> ControlFlow<()>,
{
    // Keep the source consistent and the destination writable.
    let source_locks = lock_disk_volumes(source)?;
    let destination_locks = lock_disk_volumes(destination)?;

    let result = copy_verbatim(&mut pair, new_signature, progress);
    finish(pair.destination, destination_locks, result)?;
    drop(source_locks);
    Ok(pair.source_len)
}

/// Writes the verbatim copy once both disks are locked.
fn copy_verbatim<F>(pair: &mut Pair, new_signature: bool, progress: F) -> Result<(), FormatError>
where
    F: FnMut(Progress) -> ControlFlow<()>,
{
    wipe_tail(&mut pair.destination, pair.source_len, pair.destination_len)?;
    clone::copy(
        &mut pair.source,
        &mut pair.destination,
        pair.source_len,
        progress,
    )?;
    if new_signature {
        let mut first = vec![0_u8; pair.sector as usize];
        read_at(&mut pair.destination, 0, &mut first)?;
        if clone::replace_mbr_signature(&mut first, fat32::default_volume_id()) {
            write_at(&mut pair.destination, 0, &first)?;
        }
    }
    Ok(())
}

/// Zeroes the end of the destination behind `used` bytes (at most the last
/// MiB), where a stale backup GPT would confuse Windows.
pub(super) fn wipe_tail(destination: &mut File, used: u64, len: u64) -> io::Result<()> {
    let start = used.max(len.saturating_sub(TAIL));
    if start >= len {
        return Ok(());
    }
    let zeros = vec![0_u8; usize::try_from(len - start).map_err(io::Error::other)?];
    write_at(destination, start, &zeros)
}

/// Flushes the destination, makes Windows re-read its partition table while
/// the old volumes are still locked, then releases the locks.
pub(super) fn finish<T>(
    destination: File,
    locks: Vec<Volume>,
    result: Result<T, FormatError>,
) -> Result<T, FormatError> {
    let synced = destination.sync_all();
    let _ = ioctl_none(&destination, IOCTL_DISK_UPDATE_PROPERTIES);
    drop(destination);
    drop(locks);
    let value = result?;
    synced?;
    Ok(value)
}

/// Reads `buf.len()` bytes at the absolute byte `offset`.
pub(super) fn read_at(file: &mut File, offset: u64, buf: &mut [u8]) -> io::Result<()> {
    file.seek(SeekFrom::Start(offset))?;
    file.read_exact(buf)
}

/// Writes `buf` at the absolute byte `offset`.
pub(super) fn write_at(file: &mut File, offset: u64, buf: &[u8]) -> io::Result<()> {
    file.seek(SeekFrom::Start(offset))?;
    file.write_all(buf)
}
