//! Small helpers for paths: `/`-separated paths on the server and file
//! names that must be valid on Windows.

use std::path::Path;

/// Joins a directory path and a name with `/`.
#[must_use]
pub fn join(parent: &str, name: &str) -> String {
    if parent.ends_with('/') {
        format!("{parent}{name}")
    } else {
        format!("{parent}/{name}")
    }
}

/// Returns the parent of a `/`-separated path (`/` for top-level entries).
#[must_use]
pub fn parent(path: &str) -> String {
    match path.trim_end_matches('/').rfind('/') {
        Some(0) | None => "/".to_owned(),
        Some(index) => path[..index].to_owned(),
    }
}

/// Adds the path to an I/O error.
pub(crate) fn with_path(err: std::io::Error, path: &Path) -> std::io::Error {
    std::io::Error::new(err.kind(), format!("{}: {err}", path.display()))
}

/// Replaces characters that Windows does not accept in file names.
///
/// `.` and `..` become `_` so that a name can never leave its directory.
#[must_use]
pub fn sanitize_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') {
                '_'
            } else {
                c
            }
        })
        .collect();
    let trimmed = cleaned.trim_end_matches([' ', '.']);
    if trimmed.is_empty() {
        "_".to_owned()
    } else {
        trimmed.to_owned()
    }
}
