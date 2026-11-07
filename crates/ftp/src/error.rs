//! Errors of the FTP client.

use std::error::Error;
use std::fmt;
use std::io;

/// Errors that can occur while talking to an FTP server.
#[derive(Debug)]
pub enum FtpError {
    /// A network or local file error.
    Io(io::Error),
    /// The server answered with an unexpected reply (code and text).
    Reply(u16, String),
    /// The server broke the protocol, or a request was refused locally
    /// (what is attached).
    Protocol(&'static str),
    /// The operation was cancelled by the progress callback.
    Cancelled,
}

impl fmt::Display for FtpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(err) => write!(f, "I/O error: {err}"),
            Self::Reply(code, text) => write!(f, "FTP server replied {code}: {text}"),
            Self::Protocol(what) => write!(f, "FTP error: {what}"),
            Self::Cancelled => f.write_str("the operation was cancelled"),
        }
    }
}

impl Error for FtpError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(err) => Some(err),
            _ => None,
        }
    }
}

impl From<io::Error> for FtpError {
    fn from(err: io::Error) -> Self {
        Self::Io(err)
    }
}
