//! Step 2 of a download: receiving the planned files.
//!
//! [`DownloadPlan::download`] puts the files in a queue, largest first, and
//! starts `workers` connections that each take the next file until the
//! queue is empty. The caller follows the transfer through
//! [`TransferEvent`]s and can cancel from them. When one connection fails,
//! all stop and the partly written file is removed.

use std::collections::VecDeque;
use std::fs::{self, File};
use std::io::BufWriter;
use std::ops::ControlFlow;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;

use crate::paths::with_path;
use crate::plan::{DownloadPlan, ExistingFiles, PlannedFile};
use crate::{FtpClient, FtpError};

/// Most connections opened at once.
pub const MAX_WORKERS: usize = 8;

/// Size of the write buffer of each connection.
const WRITE_BUFFER: usize = 256 * 1024;

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
}

/// What happens during [`FtpClient::download_parallel`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferEvent<'a> {
    /// The directories were walked; these files will be downloaded.
    Planned(&'a [PlannedFile]),
    /// Connection `worker` started downloading `file`.
    Started {
        /// Connection index.
        worker: usize,
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
    /// Connection `worker` completed `file` (`received` bytes).
    Finished {
        /// Connection index.
        worker: usize,
        /// The file.
        file: &'a PlannedFile,
        /// Bytes received.
        received: u64,
    },
}

impl DownloadPlan {
    /// Downloads the files over `workers` connections to `host`.
    ///
    /// `existing` decides about files already on the PC; a file whose
    /// download fails is removed. `events` follows the transfer and may
    /// cancel it.
    ///
    /// # Errors
    ///
    /// Returns the first error of any connection: [`FtpError::Io`] when a
    /// local file already exists (with [`ExistingFiles::Fail`]) or cannot be
    /// written, [`FtpError::Cancelled`] when `events` cancelled, and any
    /// error of downloading.
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
        let mut planned = Vec::new();
        let mut locals = Vec::new();
        for ((file, local), present) in self.files.iter().zip(&self.locals).zip(&self.existing) {
            match (present, existing) {
                (Some(_), ExistingFiles::Fail) => {
                    return Err(with_path(std::io::ErrorKind::AlreadyExists.into(), local).into());
                }
                (Some(size), ExistingFiles::SkipComplete) if *size == file.size => {
                    stats.existing += 1;
                }
                _ => {
                    planned.push(file.clone());
                    locals.push(local.clone());
                }
            }
        }
        if events(TransferEvent::Planned(&planned)).is_break() {
            return Err(FtpError::Cancelled);
        }

        // Largest first: long transfers start early and the end stays even.
        let mut jobs: Vec<Job> = locals
            .into_iter()
            .enumerate()
            .map(|(index, local)| Job { index, local })
            .collect();
        jobs.sort_by_key(|job| std::cmp::Reverse(planned[job.index].size));
        let workers = workers.clamp(1, MAX_WORKERS).min(jobs.len().max(1));
        let shared = Shared {
            files: &planned,
            queue: Mutex::new(jobs.into()),
            overwrite: existing != ExistingFiles::Fail,
            stop: AtomicBool::new(false),
            error: Mutex::new(None),
            events: &events,
        };
        let results: Vec<(u64, u64)> = thread::scope(|scope| {
            let handles: Vec<_> = (0..workers)
                .map(|worker_index| {
                    let shared = &shared;
                    scope.spawn(move || worker(host, worker_index, shared))
                })
                .collect();
            handles
                .into_iter()
                .map(|h| h.join().unwrap_or((0, 0)))
                .collect()
        });

        if let Some(err) = shared.error.into_inner().ok().flatten() {
            return Err(err);
        }
        for (files, bytes) in results {
            stats.files += files;
            stats.bytes += bytes;
        }
        Ok(stats)
    }
}

/// A file to download.
struct Job {
    /// Index into the planned files.
    index: usize,
    /// Path on the PC.
    local: PathBuf,
}

/// State shared by the workers.
struct Shared<'a, F> {
    /// The planned files.
    files: &'a [PlannedFile],
    /// Files still to download.
    queue: Mutex<VecDeque<Job>>,
    /// Whether existing local files are replaced.
    overwrite: bool,
    /// Set when a worker failed or the user cancelled.
    stop: AtomicBool,
    /// First error.
    error: Mutex<Option<FtpError>>,
    /// Event callback.
    events: &'a F,
}

impl<F: Fn(TransferEvent<'_>) -> ControlFlow<()> + Sync> Shared<'_, F> {
    /// Records the first error and stops the other workers.
    fn fail(&self, err: FtpError) {
        self.stop.store(true, Ordering::Relaxed);
        if let Ok(mut slot) = self.error.lock() {
            if slot.is_none() || matches!(slot.as_ref(), Some(FtpError::Cancelled)) {
                *slot = Some(err);
            }
        }
    }

    /// Reports `event` and asks whether to go on.
    fn report(&self, event: TransferEvent<'_>) -> ControlFlow<()> {
        if self.stop.load(Ordering::Relaxed) {
            return ControlFlow::Break(());
        }
        (self.events)(event)
    }

    /// Takes the next file, unless stopped.
    fn next(&self) -> Option<Job> {
        if self.stop.load(Ordering::Relaxed) {
            return None;
        }
        self.queue.lock().ok()?.pop_front()
    }
}

/// Downloads files from the queue on a connection of its own; returns the
/// files and bytes it completed.
fn worker<F: Fn(TransferEvent<'_>) -> ControlFlow<()> + Sync>(
    host: &str,
    index: usize,
    shared: &Shared<'_, F>,
) -> (u64, u64) {
    let mut client = match FtpClient::connect(host) {
        Ok(client) => client,
        Err(err) => {
            shared.fail(err);
            return (0, 0);
        }
    };
    let (mut files, mut bytes) = (0, 0);
    while let Some(job) = shared.next() {
        match download_one(&mut client, index, &job, shared) {
            Ok(n) => {
                files += 1;
                bytes += n;
            }
            Err(err) => {
                shared.fail(err);
                break;
            }
        }
    }
    if !shared.stop.load(Ordering::Relaxed) {
        client.quit();
    }
    (files, bytes)
}

/// Downloads one file; removes it again when the download fails.
fn download_one<F: Fn(TransferEvent<'_>) -> ControlFlow<()> + Sync>(
    client: &mut FtpClient,
    worker: usize,
    job: &Job,
    shared: &Shared<'_, F>,
) -> Result<u64, FtpError> {
    let file = &shared.files[job.index];
    if shared
        .report(TransferEvent::Started { worker, file })
        .is_break()
    {
        return Err(FtpError::Cancelled);
    }
    let local = if shared.overwrite {
        File::create(&job.local)
    } else {
        File::create_new(&job.local)
    }
    .map_err(|e| with_path(e, &job.local))?;
    let mut out = BufWriter::with_capacity(WRITE_BUFFER, local);
    let result = client.retrieve(&file.remote, &mut out, |received| {
        shared.report(TransferEvent::Progress { worker, received })
    });
    let result = result.and_then(|received| {
        out.into_inner()
            .map_err(std::io::IntoInnerError::into_error)?;
        Ok(received)
    });
    match result {
        Ok(received) => {
            // The file is complete: a cancel request applies to the next one.
            let _ = shared.report(TransferEvent::Finished {
                worker,
                file,
                received,
            });
            Ok(received)
        }
        Err(err) => {
            let _ = fs::remove_file(&job.local);
            Err(err)
        }
    }
}
