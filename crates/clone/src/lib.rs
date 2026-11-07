//! Copying disks, partitions and directory trees.
//!
//! * [`copy`] streams a byte range from a source to a destination with the
//!   read and the write running in parallel; the buffers are recycled, so no
//!   memory is allocated per chunk.
//! * [`copy_tree`] / [`tree_size`] copy the files of a mounted volume.
//! * [`replace_mbr_signature`] gives a clone a fresh disk signature.

mod signature;
mod stream;
mod tree;

#[cfg(test)]
mod tests;

pub use signature::replace_mbr_signature;
pub use stream::copy;
pub use tree::{copy_tree, tree_size};

/// Size of one transfer buffer (8 MiB, a multiple of every sector size).
pub const CHUNK_SIZE: usize = 8 * 1024 * 1024;
