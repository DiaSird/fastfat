//! Cloning with a rebuilt partition table: a new FAT32 file system filled
//! with the files of the source, plus verbatim or empty other partitions.

use std::fs::File;
use std::io;
use std::ops::ControlFlow;
use std::path::Path;

use super::clone::{CloneMode, CloneOptions, Pair, finish, read_at, wipe_tail, write_at};
use super::ioctl::context;
use super::volume::{find_volume_root, lock_disk_volumes, wait_for_volume_root};
use fastfat_clone as clone;
use fastfat_fat32::{
    self as fat32, FormatError, FormatOptions, Geometry, Layout, Progress, VolumeLabel,
};
use fastfat_mbr::{self as mbr, PartitionEntry, PartitionView};
use fastfat_units::format_size;

/// Top-level folders that Windows maintains itself and that are not copied.
const SKIPPED_FOLDERS: [&str; 2] = ["System Volume Information", "$RECYCLE.BIN"];

/// Bytes zeroed at the start of an empty partition, so that the console
/// finds no stale file system there and offers to format it.
const FRESH_WIPE: usize = 4 * 1024 * 1024;

/// Partition tables of a rebuilt clone.
struct Rebuild {
    /// Entries of the source.
    entries: [PartitionEntry; 4],
    /// Entries of the destination.
    planned: [PartitionEntry; 4],
    /// Index of the FAT32 entry in `entries`.
    source_fat: usize,
    /// Index of the FAT32 entry in `planned`.
    destination_fat: usize,
    /// Whether the other partitions are copied (otherwise they are created
    /// empty).
    copy_others: bool,
    /// MBR sector of the source, rewritten with the planned entries.
    table: Vec<u8>,
}

impl Rebuild {
    /// Reads the source table and plans the destination layout for `mode`.
    fn plan(pair: &mut Pair, mode: CloneMode) -> Result<Self, FormatError> {
        if pair.sector != mbr::SECTOR_SIZE {
            return Err(FormatError::UnsupportedPartitionTable(
                "only disks with 512 byte sectors can be rebuilt",
            ));
        }
        let mut table = vec![0_u8; mbr::SECTOR_SIZE as usize];
        read_at(&mut pair.source, 0, &mut table)?;
        let entries = mbr::parse_partitions(&table).ok_or(
            FormatError::UnsupportedPartitionTable("the source is not an MBR disk"),
        )?;
        let disk_sectors = pair.destination_len / u64::from(mbr::SECTOR_SIZE);

        let (source_fat, destination_fat, planned, copy_others) = match mode {
            CloneMode::WiiUFreshStorage { storage_bytes } => {
                let storage = storage_bytes.div_ceil(u64::from(mbr::SECTOR_SIZE));
                let (fat, planned) = mbr::plan_fresh_storage(&entries, disk_sectors, storage)?;
                (fat, 0, planned, false)
            }
            CloneMode::ExpandFat32 | CloneMode::Verbatim => {
                let (fat, planned) = mbr::plan_expansion(&entries, disk_sectors)?;
                (fat, fat, planned, true)
            }
        };
        Ok(Self {
            entries,
            planned,
            source_fat,
            destination_fat,
            copy_others,
            table,
        })
    }

    /// Returns the slots copied verbatim (same slot on both disks).
    fn verbatim(&self) -> impl Iterator<Item = usize> + '_ {
        (0..self.entries.len())
            .filter(|&i| self.copy_others && i != self.source_fat && !self.entries[i].is_empty())
    }

    /// Returns the slots of the destination that are created empty.
    fn fresh(&self) -> impl Iterator<Item = usize> + '_ {
        (0..self.planned.len()).filter(|&i| {
            !self.copy_others && i != self.destination_fat && !self.planned[i].is_empty()
        })
    }

    /// Returns the FAT32 partition of the destination.
    const fn new_fat(&self) -> PartitionEntry {
        self.planned[self.destination_fat]
    }

    /// Writes the destination: verbatim or empty partitions, an empty FAT32
    /// file system and finally the new partition table.
    fn write<F>(
        &mut self,
        pair: &mut Pair,
        options: &FormatOptions,
        new_signature: bool,
        report: &mut F,
    ) -> Result<(), FormatError>
    where
        F: FnMut(u64) -> ControlFlow<()>,
    {
        let end = self
            .planned
            .iter()
            .map(PartitionEntry::end)
            .max()
            .unwrap_or(0)
            * u64::from(mbr::SECTOR_SIZE);
        wipe_tail(&mut pair.destination, end, pair.destination_len)?;
        for index in self.verbatim() {
            copy_partition(pair, self.entries[index], self.planned[index], report)?;
        }
        let zeros = vec![0_u8; FRESH_WIPE];
        for index in self.fresh() {
            let entry = self.planned[index];
            let len = usize::try_from(entry.len()).map_or(FRESH_WIPE, |len| len.min(FRESH_WIPE));
            write_at(&mut pair.destination, entry.offset(), &zeros[..len])?;
        }
        let mut view = PartitionView::new(&mut pair.destination, self.new_fat().offset());
        fat32::format(
            &mut view,
            &partition_geometry(self.new_fat()),
            options,
            |_| ControlFlow::Continue(()),
        )?;

        mbr::write_partitions(&mut self.table, &self.planned);
        if new_signature {
            clone::replace_mbr_signature(&mut self.table, fat32::default_volume_id());
        }
        write_at(&mut pair.destination, 0, &self.table)?;
        Ok(())
    }
}

/// Clones the source with a rebuilt partition table (see [`CloneMode`]).
///
/// The files of the FAT32 partition are copied from the mounted source
/// volume, which is therefore not locked.
pub(super) fn clone_rebuilt<F>(
    mut pair: Pair,
    source: u32,
    destination: u32,
    options: CloneOptions,
    mut progress: F,
) -> Result<u64, FormatError>
where
    F: FnMut(Progress) -> ControlFlow<()>,
{
    let mut rebuild = Rebuild::plan(&mut pair, options.mode)?;
    let old_fat = rebuild.entries[rebuild.source_fat];
    let new_fat = rebuild.new_fat();

    let source_root = find_volume_root(source, old_fat.offset())?
        .ok_or_else(|| io::Error::other("the FAT32 partition of the source is not mounted"))?;
    let file_bytes = clone::tree_size(Path::new(&source_root), &SKIPPED_FOLDERS)?;
    let capacity = Layout::compute(&partition_geometry(new_fat), None)?.data_bytes();
    if capacity < file_bytes {
        return Err(io::Error::other(format!(
            "the new FAT32 partition ({}) cannot hold the files of the source ({})",
            format_size(capacity),
            format_size(file_bytes)
        ))
        .into());
    }

    let total = rebuild
        .verbatim()
        .map(|i| rebuild.entries[i].len())
        .sum::<u64>()
        + file_bytes;
    let mut done = 0_u64;
    let mut report = |bytes: u64| {
        done += bytes;
        progress(Progress {
            written: done,
            total,
        })
    };

    let format_options = source_format_options(&mut pair.source, old_fat)?;
    let destination_locks = lock_disk_volumes(destination)?;
    let result = rebuild.write(
        &mut pair,
        &format_options,
        options.new_signature,
        &mut report,
    );
    finish(pair.destination, destination_locks, result)?;

    let destination_root = wait_for_volume_root(destination, new_fat.offset())
        .map_err(|e| context(e, "cannot find the new FAT32 volume"))?;
    clone::copy_tree(
        Path::new(&source_root),
        Path::new(&destination_root),
        &SKIPPED_FOLDERS,
        &mut report,
    )?;
    Ok(total)
}

/// Returns format options that keep the label and the serial number of the
/// source FAT32 partition `entry`.
fn source_format_options(source: &mut File, entry: PartitionEntry) -> io::Result<FormatOptions> {
    let mut boot = vec![0_u8; mbr::SECTOR_SIZE as usize];
    read_at(source, entry.offset(), &mut boot)?;
    Ok(FormatOptions {
        cluster_size: None,
        label: VolumeLabel::from_boot_sector(&boot),
        full: false,
        volume_id: Some(u32::from_le_bytes([boot[67], boot[68], boot[69], boot[70]])),
    })
}

/// Returns the geometry of the FAT32 partition `entry`.
fn partition_geometry(entry: PartitionEntry) -> Geometry {
    Geometry {
        bytes_per_sector: mbr::SECTOR_SIZE,
        total_sectors: u64::from(entry.sectors),
        hidden_sectors: u64::from(entry.start),
        sectors_per_track: 63,
        heads: 255,
    }
}

/// Copies one partition verbatim from its old to its new position.
fn copy_partition<F>(
    pair: &mut Pair,
    from: PartitionEntry,
    to: PartitionEntry,
    report: &mut F,
) -> Result<(), FormatError>
where
    F: FnMut(u64) -> ControlFlow<()>,
{
    let mut source = PartitionView::new(&mut pair.source, from.offset());
    let mut destination = PartitionView::new(&mut pair.destination, to.offset());
    let mut reported = 0;
    clone::copy(&mut source, &mut destination, from.len(), |p| {
        let delta = p.written - reported;
        reported = p.written;
        report(delta)
    })
}
