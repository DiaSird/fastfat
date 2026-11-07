//! A window into a device that starts at a partition offset.

use std::io::{self, Read, Seek, SeekFrom, Write};

/// A window into a device that starts at a fixed byte offset, so that a
/// partition can be addressed from 0.
///
/// The view keeps its own position and re-seeks the device before every
/// read or write, so the device position at creation time does not matter.
/// [`SeekFrom::End`] is not supported because the end of the partition is not
/// known to the view.
#[derive(Debug)]
pub struct PartitionView<'a, D: ?Sized> {
    inner: &'a mut D,
    base: u64,
    pos: u64,
}

impl<'a, D: ?Sized> PartitionView<'a, D> {
    /// Creates a view of `inner` starting at byte `base`, positioned at the
    /// start of the partition.
    pub const fn new(inner: &'a mut D, base: u64) -> Self {
        Self {
            inner,
            base,
            pos: 0,
        }
    }

    /// Returns the absolute device offset of the current position.
    fn absolute(&self) -> io::Result<u64> {
        self.base.checked_add(self.pos).ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "offset overflows the device")
        })
    }
}

impl<D: Write + Seek + ?Sized> Write for PartitionView<'_, D> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.inner.seek(SeekFrom::Start(self.absolute()?))?;
        let n = self.inner.write(buf)?;
        self.pos += n as u64;
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

impl<D: Read + Seek + ?Sized> Read for PartitionView<'_, D> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.inner.seek(SeekFrom::Start(self.absolute()?))?;
        let n = self.inner.read(buf)?;
        self.pos += n as u64;
        Ok(n)
    }
}

impl<D: ?Sized> Seek for PartitionView<'_, D> {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        let new = match pos {
            SeekFrom::Start(offset) => Some(offset),
            SeekFrom::Current(delta) => self.pos.checked_add_signed(delta),
            SeekFrom::End(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "seeking from the end of a partition",
                ));
            }
        };
        self.pos = new.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "seek before the start of the partition",
            )
        })?;
        Ok(self.pos)
    }
}
