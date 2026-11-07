//! What the transfer thread does: walk the folders on the server, then
//! download, skipping or replacing files already on the PC as chosen before
//! the transfer.

use std::ops::ControlFlow;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use fastfat_ftp::{CopyStats, FtpClient, FtpError};

use super::TransferRequest;
use super::progress::TransferView;

/// Runs a download on the transfer thread with connections of its own.
pub(super) fn run(
    request: &TransferRequest,
    view: &Mutex<TransferView>,
    cancel: &AtomicBool,
) -> Result<CopyStats, FtpError> {
    let mut client = FtpClient::connect(&request.host)?;
    let plan = client.plan_download(&request.parent, &request.entries, &request.destination)?;
    let result = plan.download(&request.host, request.existing, request.workers, |event| {
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
