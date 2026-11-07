//! One round of a download: several connections share a queue of files.
//!
//! Each connection takes the next file (largest first, so that the round
//! ends evenly), checks whether the PC already has it (see
//! [`ExistingFiles`]), downloads it and checks its size. A file that fails goes to
//! the failed list and the connection is opened again for the next file.
//! Only a cancel request stops the whole round.

use std::collections::VecDeque;
use std::fs::{self, File};
use std::io::BufWriter;
use std::ops::ControlFlow;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;

use crate::download::{MAX_WORKERS, TransferEvent};
use crate::paths::with_path;
use crate::plan::{ExistingFiles, PlannedFile};
use crate::{FtpClient, FtpError};

/// Size of the write buffer of each connection.
const WRITE_BUFFER: usize = 256 * 1024;

/// A file to download.
pub(crate) struct Job {
    /// Index into the planned files.
    pub(crate) index: usize,
    /// Path on the PC.
    pub(crate) local: PathBuf,
}

/// Outcome of one round.
pub(crate) struct RoundResult {
    /// Files downloaded and checked.
    pub(crate) files: u64,
    /// Their bytes.
    pub(crate) bytes: u64,
    /// Files skipped because the PC already had them.
    pub(crate) existing: u64,
    /// Files that failed.
    pub(crate) failed: Vec<Job>,
    /// Whether the user cancelled.
    pub(crate) cancelled: bool,
}

/// State shared by the connections of a round.
struct Shared<'a, F> {
    /// The planned files.
    files: &'a [PlannedFile],
    /// Files still to download.
    queue: Mutex<VecDeque<Job>>,
    /// Files that failed.
    failed: Mutex<Vec<Job>>,
    /// What to do with files already on the PC.
    policy: ExistingFiles,
    /// Set when the user cancelled.
    cancelled: AtomicBool,
    /// Event callback.
    events: &'a F,
}

impl<F: Fn(TransferEvent<'_>) -> ControlFlow<()> + Sync> Shared<'_, F> {
    /// Reports `event`; a cancel request stops every connection.
    fn report(&self, event: TransferEvent<'_>) -> ControlFlow<()> {
        if self.cancelled.load(Ordering::Relaxed) || (self.events)(event).is_break() {
            self.cancelled.store(true, Ordering::Relaxed);
            return ControlFlow::Break(());
        }
        ControlFlow::Continue(())
    }

    /// Takes the next file, unless cancelled.
    fn next(&self) -> Option<Job> {
        if self.cancelled.load(Ordering::Relaxed) {
            return None;
        }
        self.queue.lock().ok()?.pop_front()
    }

    /// Puts `job` on the failed list.
    fn fail(&self, worker: usize, job: Job) {
        let _ = self.report(TransferEvent::Failed {
            worker,
            file: &self.files[job.index],
        });
        if let Ok(mut failed) = self.failed.lock() {
            failed.push(job);
        }
    }
}

/// Downloads `jobs` over `workers` connections to `host`.
pub(crate) fn run_round<F>(
    host: &str,
    files: &[PlannedFile],
    mut jobs: Vec<Job>,
    policy: ExistingFiles,
    workers: usize,
    events: &F,
) -> RoundResult
where
    F: Fn(TransferEvent<'_>) -> ControlFlow<()> + Sync,
{
    jobs.sort_by_key(|job| std::cmp::Reverse(files[job.index].size));
    let workers = workers.clamp(1, MAX_WORKERS).min(jobs.len().max(1));
    let shared = Shared {
        files,
        queue: Mutex::new(jobs.into()),
        failed: Mutex::new(Vec::new()),
        policy,
        cancelled: AtomicBool::new(false),
        events,
    };
    let totals: Vec<Totals> = thread::scope(|scope| {
        let handles: Vec<_> = (0..workers)
            .map(|index| {
                let shared = &shared;
                scope.spawn(move || worker(host, index, shared))
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().unwrap_or_default())
            .collect()
    });

    let cancelled = shared.cancelled.load(Ordering::Relaxed);
    let mut failed = shared.failed.into_inner().unwrap_or_default();
    // Files no connection could take (every connection failed) failed too.
    failed.extend(shared.queue.into_inner().unwrap_or_default());
    RoundResult {
        files: totals.iter().map(|t| t.files).sum(),
        bytes: totals.iter().map(|t| t.bytes).sum(),
        existing: totals.iter().map(|t| t.existing).sum(),
        failed,
        cancelled,
    }
}

/// What one connection did.
#[derive(Default)]
struct Totals {
    /// Files downloaded.
    files: u64,
    /// Their bytes.
    bytes: u64,
    /// Files skipped because the PC already had them.
    existing: u64,
}

/// Downloads files from the queue on a connection of its own.
fn worker<F>(host: &str, index: usize, shared: &Shared<'_, F>) -> Totals
where
    F: Fn(TransferEvent<'_>) -> ControlFlow<()> + Sync,
{
    let mut client: Option<FtpClient> = None;
    let mut totals = Totals::default();
    while let Some(job) = shared.next() {
        if already_downloaded(&job, shared) {
            totals.existing += 1;
            let _ = shared.report(TransferEvent::Skipped {
                file: &shared.files[job.index],
            });
            continue;
        }
        let connected = client.take().map_or_else(|| FtpClient::connect(host), Ok);
        let Ok(mut connection) = connected else {
            // The server is unreachable from here: leave the rest of the
            // queue to the other connections or the next round.
            shared.fail(index, job);
            break;
        };
        match download_one(&mut connection, index, &job, shared) {
            Ok(n) => {
                totals.files += 1;
                totals.bytes += n;
                client = Some(connection);
            }
            Err(FtpError::Cancelled) => break,
            // The connection may be out of step: open a new one next time.
            Err(_) => shared.fail(index, job),
        }
    }
    if let Some(client) = client {
        if !shared.cancelled.load(Ordering::Relaxed) {
            client.quit();
        }
    }
    totals
}

/// Returns whether the PC already has `job`'s file and it is to be skipped.
fn already_downloaded<F>(job: &Job, shared: &Shared<'_, F>) -> bool {
    shared.policy == ExistingFiles::SkipComplete
        && fs::metadata(&job.local)
            .is_ok_and(|m| m.is_file() && m.len() == shared.files[job.index].size)
}

/// Downloads one file and checks its size; removes it when it fails.
fn download_one<F>(
    client: &mut FtpClient,
    worker: usize,
    job: &Job,
    shared: &Shared<'_, F>,
) -> Result<u64, FtpError>
where
    F: Fn(TransferEvent<'_>) -> ControlFlow<()> + Sync,
{
    let file = &shared.files[job.index];
    if shared
        .report(TransferEvent::Started { worker, file })
        .is_break()
    {
        return Err(FtpError::Cancelled);
    }
    let result = receive(client, worker, job, file, shared).and_then(|received| {
        check_size(job, file, received)?;
        Ok(received)
    });
    match result {
        Ok(received) => {
            // The file is complete: a cancel request applies to the next one.
            let _ = (shared.events)(TransferEvent::Finished {
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

/// Receives `file` into its local path.
fn receive<F>(
    client: &mut FtpClient,
    worker: usize,
    job: &Job,
    file: &PlannedFile,
    shared: &Shared<'_, F>,
) -> Result<u64, FtpError>
where
    F: Fn(TransferEvent<'_>) -> ControlFlow<()> + Sync,
{
    // Anything still at the path is not a complete copy: replace it.
    let local = File::create(&job.local).map_err(|e| with_path(e, &job.local))?;
    let mut out = BufWriter::with_capacity(WRITE_BUFFER, local);
    let received = client.retrieve(&file.remote, &mut out, |received| {
        shared.report(TransferEvent::Progress { worker, received })
    })?;
    out.into_inner()
        .map_err(std::io::IntoInnerError::into_error)?;
    Ok(received)
}

/// Checks that the bytes received and the file on the PC have the size the
/// server listed.
fn check_size(job: &Job, file: &PlannedFile, received: u64) -> Result<(), FtpError> {
    let on_disk = fs::metadata(&job.local)
        .map_err(|e| with_path(e, &job.local))?
        .len();
    if received == file.size && on_disk == file.size {
        Ok(())
    } else {
        Err(FtpError::Protocol("the downloaded file has the wrong size"))
    }
}
