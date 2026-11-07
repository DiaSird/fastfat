//! Planning larger layouts in which the FAT32 partition grows.

use std::cmp::Reverse;

use super::{
    PARTITION_START, PARTITION_TYPE_FAT32_LBA, PARTITION_TYPE_WII_U_STORAGE, PartitionEntry,
};
use fastfat_fat32::FormatError;

/// Partition types of extended partitions, whose logical partitions are not
/// supported.
const EXTENDED_TYPES: [u8; 3] = [0x05, 0x0F, 0x85];

/// Partition alignment in sectors (1 MiB).
const ALIGN: u64 = PARTITION_START as u64;

/// Error returned when the planned partitions do not fit on the disk.
const NO_ROOM: FormatError =
    FormatError::UnsupportedPartitionTable("the partitions do not fit on the disk");

/// Plans a layout for a disk of `disk_sectors` in which the first FAT32
/// partition grows into all the extra space.
///
/// Partitions in front of the FAT32 partition keep their position.
/// Partitions behind it keep their size and order but are moved to the end
/// of the disk, each aligned to 1 MiB. At most `u32::MAX` sectors (2 TiB) are
/// used, the MBR limit. Slots keep their index in the table.
///
/// Returns the index of the FAT32 entry and the new entries.
///
/// # Errors
///
/// Returns [`FormatError::UnsupportedPartitionTable`] when there is no FAT32
/// partition, an extended partition is present, the partitions do not fit,
/// or the new FAT32 partition would be smaller than the old one.
pub fn plan_expansion(
    entries: &[PartitionEntry; 4],
    disk_sectors: u64,
) -> Result<(usize, [PartitionEntry; 4]), FormatError> {
    let fat = first_fat32(entries)?;
    let fat_start = entries[fat].start;

    // Partitions behind the FAT32 one, last first.
    let mut behind: Vec<usize> = (0..entries.len())
        .filter(|&i| !entries[i].is_empty() && entries[i].start > fat_start)
        .collect();
    behind.sort_by_key(|&i| Reverse(entries[i].start));

    let mut planned = *entries;
    let mut limit = mbr_limit(disk_sectors);
    for &index in &behind {
        let start = align_down(
            limit
                .checked_sub(u64::from(entries[index].sectors))
                .ok_or(NO_ROOM)?,
        );
        planned[index].start = u32::try_from(start).map_err(|_| NO_ROOM)?;
        limit = start;
    }

    let fat_sectors = sectors_between(fat_start, limit)?;
    if fat_sectors < entries[fat].sectors {
        return Err(FormatError::UnsupportedPartitionTable(
            "the destination has no room to grow the FAT32 partition",
        ));
    }
    planned[fat].sectors = fat_sectors;
    planned[fat].kind = PARTITION_TYPE_FAT32_LBA;
    Ok((fat, planned))
}

/// Plans the Wii U layout that `wafel_usb_partition` expects.
///
/// The FAT32 partition comes first (at the position it has on the source),
/// followed by an empty storage partition of type `0x17` with at least
/// `storage_sectors` sectors at the end of the disk, for the console to
/// format. Other partitions of the source are dropped. With
/// `storage_sectors == 0` the FAT32 partition takes the whole disk.
///
/// Returns the index of the FAT32 entry in `entries` and the new entries, in
/// which the FAT32 partition is slot 0 and the storage partition slot 1.
///
/// # Errors
///
/// Returns [`FormatError::UnsupportedPartitionTable`] when there is no FAT32
/// partition, an extended partition is present or the partitions do not fit.
pub fn plan_fresh_storage(
    entries: &[PartitionEntry; 4],
    disk_sectors: u64,
    storage_sectors: u64,
) -> Result<(usize, [PartitionEntry; 4]), FormatError> {
    let fat = first_fat32(entries)?;
    let fat_start = entries[fat].start;
    let limit = mbr_limit(disk_sectors);
    let storage_start = if storage_sectors == 0 {
        limit
    } else {
        align_down(limit.checked_sub(storage_sectors).ok_or(NO_ROOM)?)
    };

    let mut planned = [PartitionEntry::default(); 4];
    planned[0] = PartitionEntry {
        status: entries[fat].status,
        kind: PARTITION_TYPE_FAT32_LBA,
        start: fat_start,
        sectors: sectors_between(fat_start, storage_start)?,
    };
    if storage_sectors > 0 {
        planned[1] = PartitionEntry {
            status: 0,
            kind: PARTITION_TYPE_WII_U_STORAGE,
            start: u32::try_from(storage_start).map_err(|_| NO_ROOM)?,
            sectors: sectors_between(u32::try_from(storage_start).map_err(|_| NO_ROOM)?, limit)?,
        };
    }
    Ok((fat, planned))
}

/// Returns the index of the FAT32 partition with the lowest start.
fn first_fat32(entries: &[PartitionEntry; 4]) -> Result<usize, FormatError> {
    if entries
        .iter()
        .any(|e| !e.is_empty() && EXTENDED_TYPES.contains(&e.kind))
    {
        return Err(FormatError::UnsupportedPartitionTable(
            "extended partitions are not supported",
        ));
    }
    entries
        .iter()
        .enumerate()
        .filter(|(_, e)| !e.is_empty() && e.is_fat32())
        .min_by_key(|(_, e)| e.start)
        .map(|(index, _)| index)
        .ok_or(FormatError::UnsupportedPartitionTable(
            "no FAT32 partition found",
        ))
}

/// Returns the usable size of a disk in sectors (MBR addresses at most
/// `u32::MAX` sectors).
fn mbr_limit(disk_sectors: u64) -> u64 {
    disk_sectors.min(u64::from(u32::MAX))
}

/// Rounds a sector number down to the partition alignment.
const fn align_down(sector: u64) -> u64 {
    sector - sector % ALIGN
}

/// Returns the number of sectors from `start` up to `end` (exclusive).
fn sectors_between(start: u32, end: u64) -> Result<u32, FormatError> {
    end.checked_sub(u64::from(start))
        .filter(|&n| n > 0)
        .and_then(|n| u32::try_from(n).ok())
        .ok_or(NO_ROOM)
}
