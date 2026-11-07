//! Copying the files of a mounted volume.

use std::ffi::OsStr;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::ops::ControlFlow;
use std::path::Path;

use super::{CHUNK_SIZE, copy};
use fastfat_fat32::FormatError;

/// Returns the total size in bytes of the regular files below `root`,
/// ignoring the top-level entries named in `skip` (ASCII case-insensitive).
///
/// # Errors
///
/// Returns an error when a directory cannot be read.
pub fn tree_size(root: &Path, skip: &[&str]) -> io::Result<u64> {
    let mut total = 0;
    let mut pending = vec![root.to_path_buf()];
    let mut top = true;
    while let Some(dir) = pending.pop() {
        for entry in fs::read_dir(&dir)? {
            let entry = entry?;
            if top && is_skipped(&entry.file_name(), skip) {
                continue;
            }
            let file_type = entry.file_type()?;
            if file_type.is_dir() {
                pending.push(entry.path());
            } else if !file_type.is_symlink() {
                total += entry.metadata()?.len();
            }
        }
        top = false;
    }
    Ok(total)
}

/// Copies the directory tree below `source` into `destination`.
///
/// Top-level entries named in `skip` (ASCII case-insensitive) are left out,
/// symbolic links are ignored and modification times are kept. Files of at
/// least [`CHUNK_SIZE`] bytes are copied with overlapping reads and writes
/// (see [`copy`]); smaller files share one buffer.
///
/// `progress` receives the number of bytes written since its previous call.
/// Returning [`ControlFlow::Break`] cancels the copy.
///
/// # Errors
///
/// Returns [`FormatError::Io`] when a directory or file cannot be read or
/// written, and [`FormatError::Cancelled`] when the callback cancelled.
pub fn copy_tree<F>(
    source: &Path,
    destination: &Path,
    skip: &[&str],
    mut progress: F,
) -> Result<(), FormatError>
where
    F: FnMut(u64) -> ControlFlow<()>,
{
    let mut buffer = vec![0_u8; CHUNK_SIZE];
    copy_dir(source, destination, skip, &mut buffer, &mut progress)
}

/// Returns whether `name` is in `skip`.
fn is_skipped(name: &OsStr, skip: &[&str]) -> bool {
    skip.iter().any(|s| name.eq_ignore_ascii_case(s))
}

/// Copies one directory level and recurses into subdirectories.
fn copy_dir<F>(
    source: &Path,
    destination: &Path,
    skip: &[&str],
    buffer: &mut [u8],
    progress: &mut F,
) -> Result<(), FormatError>
where
    F: FnMut(u64) -> ControlFlow<()>,
{
    fs::create_dir_all(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let name = entry.file_name();
        if is_skipped(&name, skip) {
            continue;
        }
        let file_type = entry.file_type()?;
        let target = destination.join(&name);
        if file_type.is_dir() {
            copy_dir(&entry.path(), &target, &[], buffer, progress)?;
        } else if !file_type.is_symlink() {
            copy_file(&entry.path(), &target, buffer, progress)?;
        }
    }
    Ok(())
}

/// Copies one file and its modification time.
fn copy_file<F>(
    source: &Path,
    destination: &Path,
    buffer: &mut [u8],
    progress: &mut F,
) -> Result<(), FormatError>
where
    F: FnMut(u64) -> ControlFlow<()>,
{
    let mut input = File::open(source)?;
    let metadata = input.metadata()?;
    let mut output = File::create(destination)?;
    let len = metadata.len();

    if len >= CHUNK_SIZE as u64 {
        let mut reported = 0;
        copy(&mut input, &mut output, len, |p| {
            let delta = p.written - reported;
            reported = p.written;
            progress(delta)
        })?;
    } else {
        loop {
            let n = input.read(buffer)?;
            if n == 0 {
                break;
            }
            output.write_all(&buffer[..n])?;
            if progress(n as u64).is_break() {
                return Err(FormatError::Cancelled);
            }
        }
    }
    if let Ok(modified) = metadata.modified() {
        let _ = output.set_modified(modified);
    }
    Ok(())
}
