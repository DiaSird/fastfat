//! Step 1 of a download: walking the folders on the server.
//!
//! [`FtpClient::plan_download`] lists every file to download (folders with
//! everything below them), creates the matching folders on the PC and notes
//! which files already exist there. The result, a [`DownloadPlan`], lets the
//! caller ask the user what to do with those files before anything is
//! downloaded (step 2, see [`crate::download`]).

use std::fs;
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};

use crate::download::{CopyStats, TransferEvent};
use crate::paths::{join, sanitize_name, with_path};
use crate::{FtpClient, FtpEntry, FtpEntryKind, FtpError};

/// Deepest folder nesting downloaded (protects against loops).
const MAX_DEPTH: usize = 64;

/// A file of a transfer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedFile {
    /// Path on the server.
    pub remote: String,
    /// Size reported by the listing.
    pub size: u64,
}

/// What to do with files that already exist on the PC.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExistingFiles {
    /// Fail before downloading anything.
    Fail,
    /// Skip files of the same size (taken as downloaded already) and
    /// download the others again (taken as incomplete).
    SkipComplete,
    /// Download and overwrite every file.
    Overwrite,
}

/// The files of a download, built by [`FtpClient::plan_download`].
#[derive(Debug, Clone, Default)]
pub struct DownloadPlan {
    /// Files on the server.
    pub(crate) files: Vec<PlannedFile>,
    /// Their paths on the PC.
    pub(crate) locals: Vec<PathBuf>,
    /// Size of the local file, for files that already exist.
    pub(crate) existing: Vec<Option<u64>>,
    /// Links that are skipped.
    pub(crate) links: u64,
}

impl DownloadPlan {
    /// Returns the files to download.
    #[must_use]
    pub fn files(&self) -> &[PlannedFile] {
        &self.files
    }

    /// Returns how many files already exist on the PC, and how many of them
    /// have the size of the file on the server.
    #[must_use]
    pub fn existing(&self) -> (u64, u64) {
        let mut all = 0;
        let mut complete = 0;
        for (file, local) in self.files.iter().zip(&self.existing) {
            if let Some(size) = local {
                all += 1;
                complete += u64::from(*size == file.size);
            }
        }
        (all, complete)
    }
}

impl FtpClient {
    /// Downloads `entries` of the directory `parent` into the existing local
    /// directory `destination` over `workers` connections to `host`, never
    /// overwriting (see [`Self::plan_download`] and [`DownloadPlan::download`]).
    ///
    /// # Errors
    ///
    /// Returns any error of [`Self::plan_download`] and
    /// [`DownloadPlan::download`].
    pub fn download_parallel<F>(
        &mut self,
        host: &str,
        parent: &str,
        entries: &[FtpEntry],
        destination: &Path,
        workers: usize,
        events: F,
    ) -> Result<CopyStats, FtpError>
    where
        F: Fn(TransferEvent<'_>) -> ControlFlow<()> + Sync,
    {
        let plan = self.plan_download(parent, entries, destination)?;
        plan.download(host, ExistingFiles::Fail, workers, events)
    }

    /// Lists every file of `entries` (in the directory `parent`), directories
    /// with everything below them, and creates the matching folders below
    /// the local directory `destination`.
    ///
    /// # Errors
    ///
    /// Returns any error of listing, and [`FtpError::Io`] when a local folder
    /// cannot be created.
    pub fn plan_download(
        &mut self,
        parent: &str,
        entries: &[FtpEntry],
        destination: &Path,
    ) -> Result<DownloadPlan, FtpError> {
        let mut plan = DownloadPlan::default();
        for entry in entries {
            self.plan(&join(parent, &entry.name), entry, destination, 0, &mut plan)?;
        }
        plan.existing = plan
            .locals
            .iter()
            .map(|local| {
                fs::metadata(local)
                    .ok()
                    .filter(std::fs::Metadata::is_file)
                    .map(|m| m.len())
            })
            .collect();
        Ok(plan)
    }

    /// Adds the files of `entry` at `path` to `plan`, creating the local
    /// directories.
    fn plan(
        &mut self,
        path: &str,
        entry: &FtpEntry,
        destination: &Path,
        depth: usize,
        plan: &mut DownloadPlan,
    ) -> Result<(), FtpError> {
        let local = destination.join(sanitize_name(&entry.name));
        match entry.kind {
            FtpEntryKind::Link => plan.links += 1,
            FtpEntryKind::File => {
                plan.files.push(PlannedFile {
                    remote: path.to_owned(),
                    size: entry.size,
                });
                plan.locals.push(local);
            }
            FtpEntryKind::Directory => {
                if depth > MAX_DEPTH {
                    return Err(FtpError::Protocol("directories nested too deep"));
                }
                match fs::create_dir(&local) {
                    Ok(()) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists && local.is_dir() => {}
                    Err(e) => return Err(with_path(e, &local).into()),
                }
                for child in self.list(path)? {
                    self.plan(&join(path, &child.name), &child, &local, depth + 1, plan)?;
                }
            }
        }
        Ok(())
    }
}
