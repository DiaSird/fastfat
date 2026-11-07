//! Streaming copy with overlapping reads and writes.

use std::io::{self, Read, Seek, SeekFrom, Write};
use std::ops::ControlFlow;
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::thread;

use super::CHUNK_SIZE;
use fastfat_fat32::{FormatError, Progress};

/// Number of buffers in flight between the reader and the writer.
const BUFFER_COUNT: usize = 3;

/// A filled buffer and the number of valid bytes in it.
type Chunk = (Vec<u8>, usize);

/// Copies the first `len` bytes of `source` to the start of `destination`.
///
/// A reader thread fills buffers while the calling thread writes the previous
/// ones, so the copy runs at the speed of the slower device instead of the
/// sum of both. `len` should be a multiple of the sector size when raw disks
/// are used.
///
/// `progress` is called after each chunk is written. Returning
/// [`ControlFlow::Break`] cancels the copy.
///
/// # Errors
///
/// Returns [`FormatError::Io`] when a read or write fails (including a source
/// shorter than `len`) and [`FormatError::Cancelled`] when the callback
/// cancelled the copy.
pub fn copy<S, D, F>(
    source: &mut S,
    destination: &mut D,
    len: u64,
    mut progress: F,
) -> Result<(), FormatError>
where
    S: Read + Seek + Send + ?Sized,
    D: Write + Seek + ?Sized,
    F: FnMut(Progress) -> ControlFlow<()>,
{
    let (full_tx, full_rx) = sync_channel::<Chunk>(BUFFER_COUNT);
    let (empty_tx, empty_rx) = sync_channel::<Vec<u8>>(BUFFER_COUNT);
    let chunk = usize::try_from(len).map_or(CHUNK_SIZE, |len| len.min(CHUNK_SIZE));
    for _ in 0..BUFFER_COUNT {
        // The receiver is alive, so these sends cannot fail.
        let _ = empty_tx.send(vec![0_u8; chunk]);
    }

    thread::scope(|scope| {
        let reader = scope.spawn(move || read_side(source, len, &empty_rx, &full_tx));
        // `write_side` owns its channel ends: when it returns early they are
        // dropped, which makes the reader stop as well.
        let written = write_side(destination, len, full_rx, empty_tx, &mut progress);
        let read = reader
            .join()
            .unwrap_or_else(|_| Err(io::Error::other("the reader thread panicked")));

        // A write error or a cancellation explains a reader that stopped early,
        // so it takes precedence over the read result.
        let written = written?;
        read?;
        if written == len {
            Ok(())
        } else {
            Err(io::Error::new(io::ErrorKind::UnexpectedEof, "the copy ended early").into())
        }
    })
}

/// Reads `len` bytes into recycled buffers and passes them to the writer.
///
/// Returns early without error when the writer has stopped.
fn read_side<S: Read + Seek + ?Sized>(
    source: &mut S,
    len: u64,
    empty: &Receiver<Vec<u8>>,
    full: &SyncSender<Chunk>,
) -> io::Result<()> {
    source.seek(SeekFrom::Start(0))?;
    let mut remaining = len;
    while remaining > 0 {
        let Ok(mut buffer) = empty.recv() else {
            return Ok(());
        };
        let n = usize::try_from(remaining).map_or(buffer.len(), |r| r.min(buffer.len()));
        source.read_exact(&mut buffer[..n])?;
        remaining -= n as u64;
        if full.send((buffer, n)).is_err() {
            return Ok(());
        }
    }
    Ok(())
}

/// Writes the buffers produced by the reader and recycles them.
///
/// Returns the number of bytes written.
fn write_side<D, F>(
    destination: &mut D,
    len: u64,
    full: Receiver<Chunk>,
    empty: SyncSender<Vec<u8>>,
    progress: &mut F,
) -> Result<u64, FormatError>
where
    D: Write + Seek + ?Sized,
    F: FnMut(Progress) -> ControlFlow<()>,
{
    destination.seek(SeekFrom::Start(0))?;
    let mut written = 0_u64;
    for (buffer, n) in &full {
        destination.write_all(&buffer[..n])?;
        written += n as u64;
        if progress(Progress {
            written,
            total: len,
        })
        .is_break()
        {
            return Err(FormatError::Cancelled);
        }
        // The reader may already be done; then the buffer is simply dropped.
        let _ = empty.send(buffer);
    }
    destination.flush()?;
    Ok(written)
}
