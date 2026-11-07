//! MBR partition tables.
//!
//! Game consoles such as the Wii U (homebrew and vWii USB loaders) only read
//! FAT32 from a primary partition of an MBR disk with 512 byte sectors; GPT
//! disks are not recognised.
//!
//! * [`format_disk`] wipes any existing partition table (including both GPT
//!   headers), creates a single primary FAT32 (LBA) partition aligned to
//!   1 MiB and formats it.
//! * [`parse_partitions`] / [`write_partitions`] read and write the four
//!   primary entries of an existing table.
//! * [`plan_expansion`] computes a layout for a larger disk in which the
//!   FAT32 partition takes all the extra space.
//! * [`plan_fresh_storage`] computes the Wii U layout: FAT32 plus an empty
//!   storage partition for the console to format.
//! * [`PartitionView`] addresses a partition of a device from offset 0.

mod disk;
mod plan;
mod table;
mod view;

#[cfg(test)]
mod tests;

pub use disk::{format_disk, partition_sectors};
pub use plan::{plan_expansion, plan_fresh_storage};
pub use table::{PartitionEntry, build_mbr, parse_partitions, write_partitions};
pub use view::PartitionView;

/// Sector size required for MBR disks written by this module.
pub const SECTOR_SIZE: u32 = 512;

/// First sector of a new partition (1 MiB, the alignment used by Windows).
pub const PARTITION_START: u32 = 2048;

/// MBR partition type for FAT32 with LBA addressing.
pub const PARTITION_TYPE_FAT32_LBA: u8 = 0x0C;

/// MBR partition type (hidden NTFS) of the Wii U storage partition, which
/// the `wafel_usb_partition` plugin attaches as USB storage.
pub const PARTITION_TYPE_WII_U_STORAGE: u8 = 0x17;
