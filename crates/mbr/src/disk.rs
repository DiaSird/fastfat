//! Initialising a whole disk as MBR with one FAT32 partition.

use std::io::{self, Seek, SeekFrom, Write};
use std::ops::ControlFlow;

use super::table::{HEADS, SECTORS_PER_TRACK};
use super::{PARTITION_START, PartitionView, SECTOR_SIZE, build_mbr};
use fastfat_fat32::{self as fat32, FormatError, FormatOptions, Geometry, Layout, Progress};

/// Sectors wiped at the start and the end of the disk (1 MiB each), which
/// covers the MBR, both GPT headers and their partition entry arrays.
const WIPE_SECTORS: u64 = 2048;

/// Returns the partition size in sectors for a disk of `disk_sectors`.
///
/// The partition ends at most at sector `u32::MAX`, the MBR addressing limit
/// (2 TiB with 512 byte sectors); space beyond is left unused.
#[must_use]
pub fn partition_sectors(disk_sectors: u64) -> u64 {
    disk_sectors
        .min(u64::from(u32::MAX))
        .saturating_sub(u64::from(PARTITION_START))
}

/// Initialises a whole disk as MBR with one FAT32 partition.
///
/// `device` is the raw disk (512 byte sectors) and `disk_sectors` its size.
/// The partition table is written last, so an interrupted run leaves a disk
/// without any partition rather than a half-written one.
///
/// # Errors
///
/// Returns an error when the disk is too small or too large for the chosen
/// cluster size (see [`Layout::compute`]), a write fails, or the progress
/// callback cancelled the operation.
pub fn format_disk<D, F>(
    device: &mut D,
    disk_sectors: u64,
    options: &FormatOptions,
    progress: F,
) -> Result<Layout, FormatError>
where
    D: Write + Seek + ?Sized,
    F: FnMut(Progress) -> ControlFlow<()>,
{
    #[allow(clippy::cast_possible_truncation)]
    let geometry = Geometry {
        bytes_per_sector: SECTOR_SIZE,
        total_sectors: partition_sectors(disk_sectors),
        hidden_sectors: u64::from(PARTITION_START),
        sectors_per_track: SECTORS_PER_TRACK as u16,
        heads: HEADS as u16,
    };
    // Validate before anything is erased.
    let partition_len = Layout::compute(&geometry, options.cluster_size)?.total_sectors;

    // Destroy the old partition table: MBR / primary GPT at the start and the
    // backup GPT at the end of the disk.
    let wipe = WIPE_SECTORS.min(disk_sectors);
    let zeros = vec![0_u8; sector_bytes(wipe)];
    write_at(device, 0, &zeros)?;
    write_at(
        device,
        (disk_sectors - wipe) * u64::from(SECTOR_SIZE),
        &zeros,
    )?;

    let mut partition =
        PartitionView::new(device, u64::from(PARTITION_START) * u64::from(SECTOR_SIZE));
    let layout = fat32::format(&mut partition, &geometry, options, progress)?;
    debug_assert_eq!(layout.total_sectors, partition_len);

    let signature = options.volume_id.unwrap_or_else(fat32::default_volume_id);
    write_at(
        device,
        0,
        &build_mbr(PARTITION_START, partition_len, signature),
    )?;
    device.flush()?;
    Ok(layout)
}

/// Converts a sector count of at most [`WIPE_SECTORS`] to a byte length.
#[allow(clippy::cast_possible_truncation)]
const fn sector_bytes(sectors: u64) -> usize {
    (sectors * SECTOR_SIZE as u64) as usize
}

/// Writes `buf` at the absolute byte `offset`.
fn write_at<D: Write + Seek + ?Sized>(device: &mut D, offset: u64, buf: &[u8]) -> io::Result<()> {
    device.seek(SeekFrom::Start(offset))?;
    device.write_all(buf)
}
