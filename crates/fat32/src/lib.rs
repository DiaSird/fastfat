//! Platform independent FAT32 file system writer.
//!
//! This module computes a FAT32 layout for a block device and writes the
//! on-disk structures (boot sector, FSInfo sector, FAT tables and root
//! directory) to any target that implements [`std::io::Write`] and
//! [`std::io::Seek`].
//!
//! Every write issued by [`format`] is a whole multiple of the sector size and
//! starts on a sector boundary, so the target may be a raw Windows volume
//! handle as well as an ordinary image file or an in-memory buffer.
//!
//! The layout follows Microsoft's *FAT32 File System Specification*
//! (`fatgen103`).
//!
//! * [`FormatError`]: the error type shared by the whole crate.
//! * [`VolumeLabel`]: label validation and encoding.
//! * [`Layout`] / [`Geometry`]: size computations.
//! * [`format`]: writing the structures.

mod error;
mod label;
mod layout;
mod writer;

#[cfg(test)]
mod tests;

pub use error::FormatError;
pub use label::VolumeLabel;
pub use layout::{Geometry, Layout, auto_cluster_size};
pub use writer::{FormatOptions, default_volume_id, format};

/// Smallest cluster count a FAT32 volume may have.
///
/// Volumes with fewer clusters are detected as FAT12/FAT16 by drivers.
pub const MIN_CLUSTERS: u32 = 65_525;

/// Largest cluster count a FAT32 volume may have.
pub const MAX_CLUSTERS: u32 = 0x0FFF_FFF5;

/// Largest supported cluster size in bytes (64 KiB).
pub const MAX_CLUSTER_SIZE: u32 = 64 * 1024;

/// Cluster sizes offered to the user, in bytes.
pub const CLUSTER_SIZES: [u32; 8] = [512, 1024, 2048, 4096, 8192, 16_384, 32_768, 65_536];

/// Progress information passed to long running callbacks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Progress {
    /// Bytes written so far.
    pub written: u64,
    /// Total bytes that will be written.
    pub total: u64,
}

impl Progress {
    /// Returns the completed fraction in the range `0.0..=1.0`.
    #[must_use]
    #[allow(clippy::cast_precision_loss)]
    pub fn fraction(&self) -> f32 {
        if self.total == 0 {
            1.0
        } else {
            (self.written as f64 / self.total as f64) as f32
        }
    }
}
