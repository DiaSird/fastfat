//! Windows drive enumeration and raw volume / disk access.
//!
//! * [`list_drives`] / [`list_disks`]: what can be formatted or cloned.
//! * [`format_drive`]: format a volume in place.
//! * [`format_disk_mbr`]: re-initialise a whole disk as MBR + FAT32, as
//!   required by the Wii U.
//! * [`clone_disk`]: copy a whole disk verbatim, with its FAT32 partition
//!   grown, or laid out for the Wii U with an empty storage partition.
//! * [`is_elevated`] / [`relaunch_elevated`]: administrator rights.

#![cfg(windows)]

mod clone;
mod disks;
mod drives;
mod elevation;
mod format;
mod ioctl;
mod rebuild;
mod volume;

pub use clone::{CloneMode, CloneOptions, clone_disk};
pub use disks::{DiskInfo, list_disks};
pub use drives::{DriveInfo, DriveKind, list_drives};
pub use elevation::{is_elevated, relaunch_elevated};
pub use format::{format_disk_mbr, format_drive};

/// Partition table style of a disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartitionStyle {
    /// Master boot record.
    Mbr,
    /// GUID partition table (not readable by the Wii U).
    Gpt,
    /// No partition table ("super floppy").
    Raw,
}

impl PartitionStyle {
    /// Returns the short display name.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Mbr => "MBR",
            Self::Gpt => "GPT",
            Self::Raw => "RAW",
        }
    }
}
