//! The progress a transfer thread publishes: the files of the transfer, the
//! file each connection is receiving and the totals per server folder.

use std::collections::HashMap;

use fastfat_ftp::{self as ftp, TransferEvent};

use super::speed::SpeedMeter;

/// A file being downloaded.
pub(super) struct ActiveFile {
    /// Path on the server.
    pub(super) remote: String,
    /// Size in bytes.
    pub(super) size: u64,
    /// Bytes received.
    pub(super) received: u64,
}

/// Progress of one server folder.
#[derive(Default, Clone, Copy)]
pub(super) struct FolderProgress {
    /// Files of the folder in the transfer.
    pub(super) files: u64,
    /// Files completed.
    pub(super) files_done: u64,
    /// Bytes of the files.
    pub(super) bytes: u64,
    /// Bytes of the completed files.
    pub(super) bytes_done: u64,
}

/// What the transfer thread publishes.
#[derive(Default)]
pub(super) struct TransferView {
    /// Whether the file list is known.
    pub(super) planned: bool,
    /// Totals of the whole transfer.
    pub(super) total: FolderProgress,
    /// Totals per server folder.
    pub(super) folders: HashMap<String, FolderProgress>,
    /// File of each connection.
    pub(super) active: Vec<Option<ActiveFile>>,
    /// Files skipped because the PC already had them.
    pub(super) skipped: u64,
    /// Retry round in progress and its file count.
    pub(super) retry: Option<(u32, usize)>,
    /// Files that failed so far (retried later).
    pub(super) failures: u64,
    /// Transfer speed.
    pub(super) speed: SpeedMeter,
}

impl TransferView {
    /// Applies an event of the transfer.
    pub(super) fn apply(&mut self, event: TransferEvent<'_>) {
        match event {
            TransferEvent::Planned(files) => {
                self.planned = true;
                for file in files {
                    for progress in [
                        &mut self.total,
                        self.folders.entry(ftp::parent(&file.remote)).or_default(),
                    ] {
                        progress.files += 1;
                        progress.bytes += file.size;
                    }
                }
            }
            TransferEvent::Started { worker, file } => {
                if self.active.len() <= worker {
                    self.active.resize_with(worker + 1, || None);
                }
                self.active[worker] = Some(ActiveFile {
                    remote: file.remote.clone(),
                    size: file.size,
                    received: 0,
                });
            }
            TransferEvent::Progress { worker, received } => {
                if let Some(Some(active)) = self.active.get_mut(worker) {
                    active.received = received;
                }
            }
            TransferEvent::Finished {
                worker,
                file,
                received,
            } => {
                for progress in [
                    &mut self.total,
                    self.folders.entry(ftp::parent(&file.remote)).or_default(),
                ] {
                    progress.files_done += 1;
                    progress.bytes_done += received;
                }
                if let Some(slot) = self.active.get_mut(worker) {
                    *slot = None;
                }
            }
            TransferEvent::Failed { worker, .. } => {
                // Not counted as done: the retry reports it again.
                self.failures += 1;
                if let Some(slot) = self.active.get_mut(worker) {
                    *slot = None;
                }
            }
            TransferEvent::Retrying { round, files } => self.retry = Some((round, files)),
            TransferEvent::Skipped { file } => {
                // Done without a transfer: count the file, not its bytes.
                self.skipped += 1;
                for progress in [
                    &mut self.total,
                    self.folders.entry(ftp::parent(&file.remote)).or_default(),
                ] {
                    progress.files_done += 1;
                    progress.bytes = progress.bytes.saturating_sub(file.size);
                }
            }
        }
    }

    /// Bytes received of the unfinished files below `folder` (`None` for
    /// all).
    pub(super) fn in_flight(&self, folder: Option<&str>) -> u64 {
        self.active
            .iter()
            .flatten()
            .filter(|a| folder.is_none_or(|f| ftp::parent(&a.remote) == f))
            .map(|a| a.received)
            .sum()
    }
}
