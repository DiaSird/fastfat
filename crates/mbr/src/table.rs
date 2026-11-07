//! Encoding and decoding of the MBR sector and its partition entries.

use super::{PARTITION_TYPE_FAT32_LBA, SECTOR_SIZE};

/// Offset of the partition table inside the MBR.
const PARTITION_TABLE: usize = 446;

/// Offset of the disk signature inside the MBR.
const DISK_SIGNATURE: usize = 440;

/// Heads per cylinder used for CHS addresses.
pub(super) const HEADS: u32 = 255;

/// Sectors per track used for CHS addresses.
pub(super) const SECTORS_PER_TRACK: u32 = 63;

/// Partition types of protective MBRs, which mean the disk really is GPT.
const PROTECTIVE_TYPES: [u8; 1] = [0xEE];

/// One primary partition entry of an MBR.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PartitionEntry {
    /// Boot indicator (`0x80` for the active partition).
    pub status: u8,
    /// Partition type, e.g. `0x0C` for FAT32 (LBA).
    pub kind: u8,
    /// First sector.
    pub start: u32,
    /// Length in sectors.
    pub sectors: u32,
}

impl PartitionEntry {
    /// Returns whether the slot is unused.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.kind == 0 || self.sectors == 0
    }

    /// Returns whether the partition type denotes FAT32.
    #[must_use]
    pub const fn is_fat32(&self) -> bool {
        matches!(self.kind, 0x0B | PARTITION_TYPE_FAT32_LBA)
    }

    /// Returns the byte offset of the partition.
    #[must_use]
    pub const fn offset(&self) -> u64 {
        self.start as u64 * SECTOR_SIZE as u64
    }

    /// Returns the length of the partition in bytes.
    #[must_use]
    pub const fn len(&self) -> u64 {
        self.sectors as u64 * SECTOR_SIZE as u64
    }

    /// Returns the sector just past the end of the partition.
    #[must_use]
    pub const fn end(&self) -> u64 {
        self.start as u64 + self.sectors as u64
    }
}

/// Reads the four primary partition entries of an MBR sector.
///
/// Returns `None` when the sector has no boot signature or is the protective
/// MBR of a GPT disk.
#[must_use]
pub fn parse_partitions(sector: &[u8]) -> Option<[PartitionEntry; 4]> {
    if sector.len() < 512 || sector[510] != 0x55 || sector[511] != 0xAA {
        return None;
    }
    let mut entries = [PartitionEntry::default(); 4];
    for (index, entry) in entries.iter_mut().enumerate() {
        let raw = &sector[PARTITION_TABLE + 16 * index..PARTITION_TABLE + 16 * (index + 1)];
        *entry = PartitionEntry {
            status: raw[0],
            kind: raw[4],
            start: u32::from_le_bytes([raw[8], raw[9], raw[10], raw[11]]),
            sectors: u32::from_le_bytes([raw[12], raw[13], raw[14], raw[15]]),
        };
    }
    if entries.iter().any(|e| PROTECTIVE_TYPES.contains(&e.kind)) {
        return None;
    }
    Some(entries)
}

/// Writes the four primary partition entries into an MBR sector, keeping
/// the boot code and the disk signature.
pub fn write_partitions(sector: &mut [u8], entries: &[PartitionEntry; 4]) {
    for (index, entry) in entries.iter().enumerate() {
        let raw = &mut sector[PARTITION_TABLE + 16 * index..PARTITION_TABLE + 16 * (index + 1)];
        if entry.is_empty() {
            raw.fill(0);
            continue;
        }
        raw[0] = entry.status;
        raw[1..4].copy_from_slice(&chs(entry.start));
        raw[4] = entry.kind;
        raw[5..8].copy_from_slice(&chs(entry
            .start
            .saturating_add(entry.sectors.saturating_sub(1))));
        raw[8..12].copy_from_slice(&entry.start.to_le_bytes());
        raw[12..16].copy_from_slice(&entry.sectors.to_le_bytes());
    }
    sector[510] = 0x55;
    sector[511] = 0xAA;
}

/// Builds a master boot record with one primary FAT32 (LBA) partition.
#[must_use]
pub fn build_mbr(start: u32, sectors: u32, disk_signature: u32) -> [u8; 512] {
    let mut mbr = [0_u8; 512];
    mbr[DISK_SIGNATURE..DISK_SIGNATURE + 4].copy_from_slice(&disk_signature.to_le_bytes());
    let entries = [
        PartitionEntry {
            status: 0,
            kind: PARTITION_TYPE_FAT32_LBA,
            start,
            sectors,
        },
        PartitionEntry::default(),
        PartitionEntry::default(),
        PartitionEntry::default(),
    ];
    write_partitions(&mut mbr, &entries);
    mbr
}

/// Encodes `lba` as a CHS address, saturating to 1023/254/63 when it does not
/// fit (drivers then use the LBA fields).
// Every value is range checked before narrowing.
#[allow(clippy::cast_possible_truncation)]
pub(super) const fn chs(lba: u32) -> [u8; 3] {
    let cylinder = lba / (HEADS * SECTORS_PER_TRACK);
    if cylinder > 1023 {
        return [0xFE, 0xFF, 0xFF];
    }
    let head = (lba / SECTORS_PER_TRACK) % HEADS;
    let sector = lba % SECTORS_PER_TRACK + 1;
    [
        head as u8,
        (sector as u8) | (((cylinder >> 8) as u8) << 6),
        cylinder as u8,
    ]
}
