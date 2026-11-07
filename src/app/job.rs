//! Worker threads: starting jobs, reporting progress and collecting results.

use std::borrow::Cow;
use std::io;
use std::ops::ControlFlow;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use eframe::egui;
use fastfat_device::{self as device, CloneMode, CloneOptions};
use fastfat_fat32::{FormatError, FormatOptions, Layout, Progress, VolumeLabel};
use fastfat_units::format_size;

use super::format_tab::cluster_label;
use super::i18n::{t, tf};
use super::{FastFatApp, Status};

/// How often the window repaints while a job is running.
const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);

/// State shared between the GUI and the worker thread.
#[derive(Default)]
pub(super) struct Shared {
    /// Completed fraction, stored as the bits of an `f32`.
    progress: AtomicU32,
    /// Set by the GUI to request cancellation.
    cancel: AtomicBool,
}

impl Shared {
    /// Publishes `progress` and reports whether the job should go on.
    fn report(&self, progress: Progress) -> ControlFlow<()> {
        self.progress
            .store(progress.fraction().to_bits(), Ordering::Relaxed);
        if self.cancel.load(Ordering::Relaxed) {
            ControlFlow::Break(())
        } else {
            ControlFlow::Continue(())
        }
    }
}

/// What a job does.
#[derive(Clone, Copy)]
pub(super) enum Task {
    /// Formatting the volume with this drive letter.
    Format(char),
    /// Cloning disk `source` onto disk `destination`; `fresh_storage` when an
    /// empty Wii U storage partition is created.
    Clone {
        source: u32,
        destination: u32,
        fresh_storage: bool,
    },
}

/// Successful result of a job.
pub(super) enum Outcome {
    /// The volume was formatted with this layout.
    Formatted(Layout),
    /// This many bytes were cloned.
    Cloned(u64),
}

/// A job running on a worker thread.
pub(super) struct Job {
    /// What the job does.
    task: Task,
    /// Progress and cancellation flag.
    shared: Arc<Shared>,
    /// Worker thread; its return value is the job result.
    handle: JoinHandle<Result<Outcome, FormatError>>,
}

impl FastFatApp {
    /// Collects the result once the worker thread has finished.
    pub(super) fn poll_job(&mut self, ctx: &egui::Context) {
        let Some(job) = &self.job else {
            return;
        };
        if !job.handle.is_finished() {
            ctx.request_repaint_after(PROGRESS_INTERVAL);
            return;
        }
        let Some(Job { task, handle, .. }) = self.job.take() else {
            return;
        };
        let result = handle.join().unwrap_or_else(|_| {
            Err(FormatError::Io(io::Error::other(
                "the worker thread panicked",
            )))
        });
        self.status = result_status(task, result);
        self.refresh();
        self.local.reload();
    }

    /// Runs `work` on a worker thread as the job `task`.
    fn spawn_job<W>(&mut self, task: Task, work: W)
    where
        W: FnOnce(&Shared) -> Result<Outcome, FormatError> + Send + 'static,
    {
        let shared = Arc::new(Shared::default());
        // Reference-count increment only; the thread needs shared ownership.
        let worker = Arc::clone(&shared);
        match thread::Builder::new().spawn(move || work(&worker)) {
            Ok(handle) => {
                self.status = Status::None;
                self.job = Some(Job {
                    task,
                    shared,
                    handle,
                });
            }
            Err(err) => {
                self.status = Status::Error(format!("cannot start the worker thread: {err}"));
            }
        }
    }

    /// Starts formatting the selected drive.
    pub(super) fn start_format(&mut self, label: VolumeLabel) {
        let Some(letter) = self.selected else {
            return;
        };
        // `FormatOptions` is `Copy`: it is moved into the thread as is.
        let options = FormatOptions {
            cluster_size: self.cluster_size,
            label,
            full: !self.quick,
            volume_id: None,
        };
        let whole_disk = self.wii_u;
        self.spawn_job(Task::Format(letter), move |shared| {
            let report = |progress| shared.report(progress);
            let layout = if whole_disk {
                device::format_disk_mbr(letter, &options, report)
            } else {
                device::format_drive(letter, &options, report)
            }?;
            Ok(Outcome::Formatted(layout))
        });
    }

    /// Starts cloning the selected disks.
    pub(super) fn start_clone(&mut self) {
        let (Some(source), Some(destination)) = (self.clone_source, self.clone_destination) else {
            return;
        };
        let options = CloneOptions {
            new_signature: self.new_signature,
            mode: self.clone_mode(),
        };
        let fresh_storage = matches!(options.mode, CloneMode::WiiUFreshStorage { storage_bytes } if storage_bytes > 0);
        self.spawn_job(
            Task::Clone {
                source,
                destination,
                fresh_storage,
            },
            move |shared| {
                device::clone_disk(source, destination, options, |progress| {
                    shared.report(progress)
                })
                .map(Outcome::Cloned)
            },
        );
    }

    /// Draws the progress of the running job.
    pub(super) fn progress_section(&self, ui: &mut egui::Ui) {
        let Some(job) = &self.job else {
            return;
        };
        let fraction = f32::from_bits(job.shared.progress.load(Ordering::Relaxed));
        match job.task {
            Task::Format(letter) => ui.label(tf("job.formatting", &[("letter", &letter)])),
            Task::Clone {
                source,
                destination,
                ..
            } => ui.label(tf(
                "job.cloning",
                &[("source", &source), ("destination", &destination)],
            )),
        };
        ui.add(egui::ProgressBar::new(fraction).show_percentage());
        let cancelling = job.shared.cancel.load(Ordering::Relaxed);
        if ui
            .add_enabled(!cancelling, egui::Button::new(t("job.cancel")))
            .clicked()
        {
            job.shared.cancel.store(true, Ordering::Relaxed);
        }
    }
}

/// Converts the result of a job into a status message.
fn result_status(task: Task, result: Result<Outcome, FormatError>) -> Status {
    match (task, result) {
        (_, Ok(Outcome::Formatted(layout))) => Status::Success(tf(
            "job.formatted",
            &[
                ("cluster", &cluster_label(layout.cluster_size())),
                ("free", &format_size(layout.data_bytes())),
            ],
        )),
        (
            Task::Clone {
                source,
                destination,
                fresh_storage,
            },
            Ok(Outcome::Cloned(bytes)),
        ) => {
            let next = if fresh_storage {
                t("job.cloned_next")
            } else {
                ""
            };
            Status::Success(tf(
                "job.cloned",
                &[
                    ("source", &source),
                    ("destination", &destination),
                    ("next", &next),
                    ("size", &format_size(bytes)),
                ],
            ))
        }
        (_, Ok(Outcome::Cloned(_))) => Status::None,
        (Task::Format(letter), Err(FormatError::Cancelled)) => Status::Info(Cow::Owned(tf(
            "job.cancelled_format",
            &[("letter", &letter)],
        ))),
        (Task::Clone { destination, .. }, Err(FormatError::Cancelled)) => Status::Info(Cow::Owned(
            tf("job.cancelled_clone", &[("destination", &destination)]),
        )),
        (_, Err(err)) => Status::Error(describe_error(&err)),
    }
}

/// Converts an error to a message, adding a hint for common causes.
fn describe_error(err: &FormatError) -> String {
    match err {
        FormatError::Io(io) if io.kind() == io::ErrorKind::PermissionDenied => {
            tf("job.run_as_admin", &[("err", &err)])
        }
        _ => err.to_string(),
    }
}
