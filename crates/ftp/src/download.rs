//! Step 2 of a download: receiving the planned files.
//!
//! [`DownloadPlan::download`] receives the files in rounds:
//!
//! 1. Every file is downloaded once (see `workers.rs` for one round). When
//!    its turn comes, a file already on the PC is skipped or replaced as the
//!    [`ExistingFiles`] choice says. Each file is checked right after it
//!    arrives: the bytes received and the
//!    size of the file on the PC must equal the size on the server. A file
//!    that fails (network error or wrong size) does not stop the others.
//! 2. The files that failed are downloaded again, overwriting the bad copy,
//!    up to [`MAX_RETRIES`] times.
//! 3. Files that still fail are reported in [`FtpError::Incomplete`].
//!
//! The caller follows the transfer through [`TransferEvent`]s and can cancel
//! from them.

use std::ops::ControlFlow;

use crate::FtpError;
use crate::plan::{DownloadPlan, ExistingFiles, PlannedFile};
use crate::workers::{Job, run_round};

/// Most connections opened at once.
pub const MAX_WORKERS: usize = 8;

/// How often files that failed are downloaded again.
pub const MAX_RETRIES: u32 = 3;

/// Totals of a copy.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CopyStats {
    /// Bytes of file data.
    pub bytes: u64,
    /// Files.
    pub files: u64,
    /// Links that were skipped.
    pub skipped: u64,
    /// Files already on the PC with the same size, not downloaded again.
    pub existing: u64,
    /// Files that failed at first and were downloaded again successfully.
    pub retried: u64,
}

/// What happens during [`DownloadPlan::download`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferEvent<'a> {
    /// These files will be downloaded.
    Planned(&'a [PlannedFile]),
    /// Connection `worker` started downloading `file`.
    Started {
        /// Connection index.
        worker: usize,
        /// The file.
        file: &'a PlannedFile,
    },
    /// `file` is already on the PC with the right size and was skipped.
    Skipped {
        /// The file.
        file: &'a PlannedFile,
    },
    /// Connection `worker` received `received` bytes of its current file.
    Progress {
        /// Connection index.
        worker: usize,
        /// Bytes of the current file received so far.
        received: u64,
    },
    /// Connection `worker` completed `file` (`received` bytes) and its size
    /// matches the server.
    Finished {
        /// Connection index.
        worker: usize,
        /// The file.
        file: &'a PlannedFile,
        /// Bytes received.
        received: u64,
    },
    /// Connection `worker` failed to download `file`; it will be retried.
    Failed {
        /// Connection index.
        worker: usize,
        /// The file.
        file: &'a PlannedFile,
    },
    /// Retry round `round` (1-based) starts with `files` files.
    Retrying {
        /// Retry round, from 1 to [`MAX_RETRIES`].
        round: u32,
        /// Files downloaded again.
        files: usize,
    },
}

impl DownloadPlan {
    /// Downloads the files over `workers` connections to `host`, checking
    /// each file and downloading failed ones again (see the module docs).
    ///
    /// `existing` decides, file by file, about files already on the PC.
    ///
    /// # Errors
    ///
    /// Returns [`FtpError::Cancelled`] when `events` cancelled and
    /// [`FtpError::Incomplete`] with the files that still failed after
    /// [`MAX_RETRIES`] retries.
    pub fn download<F>(
        &self,
        host: &str,
        existing: ExistingFiles,
        workers: usize,
        events: F,
    ) -> Result<CopyStats, FtpError>
    where
        F: Fn(TransferEvent<'_>) -> ControlFlow<()> + Sync,
    {
        let mut stats = CopyStats {
            skipped: self.links,
            ..CopyStats::default()
        };
        let planned = &self.files;
        let mut pending: Vec<Job> = self
            .locals
            .iter()
            .enumerate()
            .map(|(index, local)| Job {
                index,
                local: local.clone(),
            })
            .collect();
        if events(TransferEvent::Planned(planned)).is_break() {
            return Err(FtpError::Cancelled);
        }

        let mut policy = existing;
        for round in 0..=MAX_RETRIES {
            if pending.is_empty() {
                break;
            }
            if round > 0 {
                let event = TransferEvent::Retrying {
                    round,
                    files: pending.len(),
                };
                if events(event).is_break() {
                    return Err(FtpError::Cancelled);
                }
                // A failed file may have been left behind: replace it.
                policy = ExistingFiles::Overwrite;
            }
            let result = run_round(host, planned, pending, policy, workers, &events);
            if result.cancelled {
                return Err(FtpError::Cancelled);
            }
            stats.files += result.files;
            stats.bytes += result.bytes;
            stats.existing += result.existing;
            if round > 0 {
                stats.retried += result.files;
            }
            pending = result.failed;
        }

        if pending.is_empty() {
            Ok(stats)
        } else {
            let files = pending
                .iter()
                .map(|job| planned[job.index].remote.clone())
                .collect();
            Err(FtpError::Incomplete(files))
        }
    }
}
