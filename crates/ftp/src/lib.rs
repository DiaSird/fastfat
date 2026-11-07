//! Read-only FTP downloads from a Wii U running the `ftpiiu` Aroma plugin
//! (any FTP server works).
//!
//! How the pieces fit together:
//!
//! 1. [`FtpClient`] (`client.rs`) is one connection: it logs in, lists
//!    folders ([`FtpEntry`], parsed in `listing.rs`) and receives files.
//! 2. [`FtpClient::plan_download`] (`plan.rs`) walks the folders to download
//!    and returns a [`DownloadPlan`] that tells which files already exist on
//!    the PC.
//! 3. [`DownloadPlan::download`] (`download.rs`) receives the files over one
//!    or more connections (`workers.rs`), checks the size of every file,
//!    downloads failed files again and reports [`TransferEvent`]s for
//!    progress displays.
//!
//! Safety: only commands that cannot change the server are ever sent (see
//! [`ALLOWED_COMMANDS`]).

mod client;
mod download;
mod error;
mod listing;
mod paths;
mod plan;
mod workers;

#[cfg(test)]
mod tests;

pub use client::FtpClient;
pub use download::{CopyStats, MAX_RETRIES, MAX_WORKERS, TransferEvent};
pub use error::FtpError;
pub use listing::{FtpEntry, FtpEntryKind, parse_list_line, parse_mlsd_line};
pub use paths::{join, parent, sanitize_name};
pub use plan::{DownloadPlan, ExistingFiles, PlannedFile};

/// Default FTP port.
pub const DEFAULT_PORT: u16 = 21;

/// Commands this client may send. Anything that writes, renames or deletes
/// on the server is absent, so the client cannot modify it.
pub const ALLOWED_COMMANDS: [&str; 9] = [
    "USER", "PASS", "TYPE", "PASV", "MLSD", "LIST", "SIZE", "RETR", "QUIT",
];
