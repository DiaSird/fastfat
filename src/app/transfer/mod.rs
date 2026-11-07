//! Background FTP downloads.
//!
//! A transfer runs on a thread of its own with its own connections, so the
//! window stays usable: browsing goes on and further downloads are queued.
//!
//! * this file: the queue ([`Transfers`]): start, poll, cancel.
//! * `worker.rs`: what the transfer thread does, including asking the user
//!   about files that already exist on the PC.
//! * `progress.rs`: the progress the thread publishes ([`progress::TransferView`]).
//! * `speed.rs`: transfer speed and remaining time.
//! * `panel.rs`: the progress panel drawn above the progress bar.

mod panel;
mod progress;
mod speed;
mod worker;

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use eframe::egui;
use fastfat_ftp::{CopyStats, ExistingFiles, FtpEntry, FtpError};

use progress::TransferView;
use worker::run;

/// How often the window repaints while a transfer runs.
const REPAINT_INTERVAL: Duration = Duration::from_millis(100);

/// A download waiting or running.
pub(super) struct TransferRequest {
    /// Address of the console.
    pub(super) host: String,
    /// Directory of the entries on the server.
    pub(super) parent: String,
    /// Entries to download.
    pub(super) entries: Vec<FtpEntry>,
    /// Existing local folder receiving them.
    pub(super) destination: PathBuf,
    /// Connections to use.
    pub(super) workers: usize,
}

/// A transfer running on its thread.
struct Running {
    /// What the thread publishes.
    view: Arc<Mutex<TransferView>>,
    /// Set to cancel.
    cancel: Arc<AtomicBool>,
    /// Answer to the question about existing files (`None` cancels).
    answer: Sender<Option<ExistingFiles>>,
    /// The thread; its value is the result.
    handle: JoinHandle<Result<CopyStats, FtpError>>,
}

/// The running transfer and the queue behind it.
#[derive(Default)]
pub(super) struct Transfers {
    /// Transfer in progress.
    running: Option<Running>,
    /// Downloads waiting for it.
    queue: VecDeque<TransferRequest>,
}

impl Transfers {
    /// Returns whether a transfer is running.
    pub(super) const fn is_busy(&self) -> bool {
        self.running.is_some()
    }

    /// Queues a download; it starts at once when nothing is running.
    pub(super) fn push(&mut self, request: TransferRequest) {
        self.queue.push_back(request);
        if self.running.is_none() {
            self.start_next();
        }
    }

    /// Starts the next queued download.
    fn start_next(&mut self) {
        let Some(request) = self.queue.pop_front() else {
            return;
        };
        let view = Arc::new(Mutex::new(TransferView::default()));
        let cancel = Arc::new(AtomicBool::new(false));
        let (answer, answers) = mpsc::channel();
        let (thread_view, thread_cancel) = (Arc::clone(&view), Arc::clone(&cancel));
        let spawned = thread::Builder::new()
            .name("ftp-transfer".to_owned())
            .spawn(move || run(&request, &thread_view, &thread_cancel, &answers));
        // Without a thread the request cannot run; report the error as the
        // result of the next poll.
        let handle = spawned.unwrap_or_else(|err| thread::spawn(move || Err(FtpError::Io(err))));
        self.running = Some(Running {
            view,
            cancel,
            answer,
            handle,
        });
    }

    /// Collects the result of a finished transfer and starts the next one.
    pub(super) fn poll(&mut self, ctx: &egui::Context) -> Option<Result<CopyStats, FtpError>> {
        let running = self.running.as_ref()?;
        if !running.handle.is_finished() {
            ctx.request_repaint_after(REPAINT_INTERVAL);
            return None;
        }
        let running = self.running.take()?;
        let result = running
            .handle
            .join()
            .unwrap_or(Err(FtpError::Protocol("the transfer thread panicked")));
        if !matches!(result, Err(FtpError::Cancelled)) {
            self.start_next();
        } else {
            // Cancelling stops the queue too.
            self.queue.clear();
        }
        Some(result)
    }
}
