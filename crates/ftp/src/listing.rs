//! Parsing directory listings (MLSD and Unix-style LIST).

/// Kind of a listed entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FtpEntryKind {
    /// A regular file.
    File,
    /// A directory.
    Directory,
    /// A symbolic link (not followed).
    Link,
}

/// A file or directory found by [`super::FtpClient::list`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FtpEntry {
    /// Name.
    pub name: String,
    /// Kind.
    pub kind: FtpEntryKind,
    /// Size in bytes (0 when unknown or for directories).
    pub size: u64,
}

/// Parses an MLSD line: `fact=value;fact=value; name`.
///
/// Fact names are case-insensitive (`ftpiiu` sends `Type=` and `Size=`).
#[must_use]
pub fn parse_mlsd_line(line: &str) -> Option<FtpEntry> {
    let line = line.trim_end_matches(['\r', '\n']);
    let (facts, name) = line.split_once(' ')?;
    if name.is_empty() {
        return None;
    }
    let mut kind = None;
    let mut size = 0;
    for fact in facts.split(';') {
        let Some((key, value)) = fact.split_once('=') else {
            continue;
        };
        if key.eq_ignore_ascii_case("type") {
            kind = Some(match value.to_ascii_lowercase().as_str() {
                "file" => FtpEntryKind::File,
                "dir" => FtpEntryKind::Directory,
                // The listed directory itself and its parent; ftpiiu names
                // the former with its full path.
                "cdir" | "pdir" => return None,
                v if v.starts_with("os.unix=slink") || v.starts_with("os.unix=symlink") => {
                    FtpEntryKind::Link
                }
                _ => return None,
            });
        } else if key.eq_ignore_ascii_case("size") {
            size = value.parse().unwrap_or(0);
        }
    }
    let kind = kind?;
    Some(FtpEntry {
        name: name.to_owned(),
        kind,
        size: if kind == FtpEntryKind::File { size } else { 0 },
    })
}

/// Parses a Unix-style LIST line:
/// `drwxr-xr-x 1 WiiU WiiU 0 Jan  1 12:00 name`.
#[must_use]
pub fn parse_list_line(line: &str) -> Option<FtpEntry> {
    let line = line.trim_end_matches(['\r', '\n']);
    let kind = match line.chars().next()? {
        'd' => FtpEntryKind::Directory,
        '-' => FtpEntryKind::File,
        'l' => FtpEntryKind::Link,
        _ => return None,
    };
    // Skip eight whitespace separated fields; the name is the rest of the
    // line and may contain spaces.
    let mut rest = line;
    let mut size = 0;
    for field in 0..8 {
        rest = rest.trim_start();
        let end = rest.find(char::is_whitespace)?;
        if field == 4 {
            size = rest[..end].parse().unwrap_or(0);
        }
        rest = &rest[end..];
    }
    let mut name = rest.trim_start();
    if kind == FtpEntryKind::Link {
        name = name.split(" -> ").next().unwrap_or(name);
    }
    if name.is_empty() {
        return None;
    }
    Some(FtpEntry {
        name: name.to_owned(),
        kind,
        size: if kind == FtpEntryKind::File { size } else { 0 },
    })
}
