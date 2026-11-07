//! What the transfer thread does: plan the download, ask about files that
//! already exist on the PC, then download.

use std::ops::ControlFlow;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::Duration;

use fastfat_ftp::{CopyStats, ExistingFiles, FtpClient, FtpError};

use super::TransferRequest;
use super::progress::TransferView;

/// Asks the user through `view` what to do with `existing` files already on
/// the PC (`complete` of them with the same size) and waits for the answer.
fn ask(
    view: &Mutex<TransferView>,
    cancel: &AtomicBool,
    answers: &Receiver<Option<ExistingFiles>>,
    existing: (u64, u64),
) -> Result<ExistingFiles, FtpError> {
    if let Ok(mut view) = view.lock() {
        view.question = Some(existing);
    }
    let answer = loop {
        match answers.recv_timeout(Duration::from_millis(100)) {
            Ok(answer) => break answer,
            Err(RecvTimeoutError::Timeout) if !cancel.load(Ordering::Relaxed) => {}
            Err(_) => break None,
        }
    };
    if let Ok(mut view) = view.lock() {
        view.question = None;
    }
    answer.ok_or(FtpError::Cancelled)
}

/// Runs a download on the transfer thread with connections of its own.
pub(super) fn run(
    request: &TransferRequest,
    view: &Mutex<TransferView>,
    cancel: &AtomicBool,
    answers: &Receiver<Option<ExistingFiles>>,
) -> Result<CopyStats, FtpError> {
    let mut client = FtpClient::connect(&request.host)?;
    let plan = client.plan_download(&request.parent, &request.entries, &request.destination)?;
    let existing = plan.existing();
    let policy = if existing.0 == 0 {
        ExistingFiles::Fail
    } else {
        ask(view, cancel, answers, existing)?
    };
    let result = plan.download(&request.host, policy, request.workers, |event| {
        if let Ok(mut view) = view.lock() {
            view.apply(event);
        }
        if cancel.load(Ordering::Relaxed) {
            ControlFlow::Break(())
        } else {
            ControlFlow::Continue(())
        }
    });
    if result.is_ok() {
        client.quit();
    }
    result
}
