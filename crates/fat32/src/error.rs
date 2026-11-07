//! Error type shared by the formatter, the partitioning and the cloning code.

use std::error::Error;
use std::fmt;
use std::io;

use super::{MAX_CLUSTERS, MIN_CLUSTERS};

/// Errors that can occur while computing a layout, formatting, partitioning
/// or cloning.
#[derive(Debug)]
pub enum FormatError {
    /// An I/O error reported by the target device.
    Io(io::Error),
    /// The sector size is not one of 512, 1024, 2048 or 4096 bytes.
    UnsupportedSectorSize(u32),
    /// The cluster size is not a power of two between the sector size and
    /// [`super::MAX_CLUSTER_SIZE`].
    InvalidClusterSize(u32),
    /// The volume has more than `u32::MAX` sectors, which FAT32 cannot address.
    VolumeTooLarge,
    /// The volume yields fewer than [`MIN_CLUSTERS`] clusters with the chosen
    /// cluster size.
    TooFewClusters(u32),
    /// The volume yields more than [`MAX_CLUSTERS`] clusters with the chosen
    /// cluster size.
    TooManyClusters(u32),
    /// The volume label violates a FAT naming rule (the reason is attached).
    InvalidLabel(&'static str),
    /// The volume label contains a character that FAT does not accept.
    InvalidLabelChar(char),
    /// The partition table cannot be handled (the reason is attached).
    UnsupportedPartitionTable(&'static str),
    /// The operation was cancelled by the progress callback.
    Cancelled,
}

impl fmt::Display for FormatError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(err) => write!(f, "I/O error: {err}"),
            Self::UnsupportedSectorSize(size) => write!(f, "unsupported sector size: {size} bytes"),
            Self::InvalidClusterSize(size) => write!(f, "invalid cluster size: {size} bytes"),
            Self::VolumeTooLarge => {
                f.write_str("volume is too large for FAT32 (more than 2^32 - 1 sectors)")
            }
            Self::TooFewClusters(count) => write!(
                f,
                "only {count} clusters fit on the volume; FAT32 needs at least {MIN_CLUSTERS} (choose a smaller cluster size)"
            ),
            Self::TooManyClusters(count) => write!(
                f,
                "{count} clusters exceed the FAT32 limit of {MAX_CLUSTERS} (choose a larger cluster size)"
            ),
            Self::InvalidLabel(reason) => write!(f, "invalid volume label: {reason}"),
            Self::InvalidLabelChar(c) => {
                write!(f, "invalid volume label: character {c:?} is not allowed")
            }
            Self::UnsupportedPartitionTable(reason) => {
                write!(f, "unsupported partition table: {reason}")
            }
            Self::Cancelled => f.write_str("the operation was cancelled"),
        }
    }
}

impl Error for FormatError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(err) => Some(err),
            _ => None,
        }
    }
}

impl From<io::Error> for FormatError {
    fn from(err: io::Error) -> Self {
        Self::Io(err)
    }
}
