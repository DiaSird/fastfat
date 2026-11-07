//! The FTP connection: logging in, listing folders and receiving files.
//!
//! This is the only place that talks the FTP protocol. Every command goes
//! through [`FtpClient::command`], which refuses anything outside
//! [`ALLOWED_COMMANDS`](crate::ALLOWED_COMMANDS), so the client cannot change
//! the console. Data is received in passive mode and binary type.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{IpAddr, SocketAddr, TcpStream, ToSocketAddrs};
use std::ops::ControlFlow;
use std::time::Duration;

use crate::listing::{FtpEntry, FtpEntryKind, parse_list_line, parse_mlsd_line};
use crate::{ALLOWED_COMMANDS, DEFAULT_PORT, FtpError};

/// Timeout for connecting and for every read and write.
const TIMEOUT: Duration = Duration::from_secs(30);

/// Timeout for establishing the control connection.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// Size of the transfer buffer.
const BUFFER: usize = 256 * 1024;

/// A reply of the server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Reply {
    /// Three-digit code.
    code: u16,
    /// Text of the last line.
    text: String,
}

/// A connection to an FTP server.
#[derive(Debug)]
pub struct FtpClient {
    /// Control connection, read side.
    reader: BufReader<TcpStream>,
    /// Control connection, write side.
    writer: TcpStream,
    /// Address of the server, used for passive data connections.
    peer: IpAddr,
    /// Whether the server answered MLSD (machine readable listings).
    mlsd: bool,
}

impl FtpClient {
    /// Connects to `host` (`address` or `address:port`) and logs in with
    /// empty credentials (as `ftpiiu` expects) or as `anonymous`.
    ///
    /// # Errors
    ///
    /// Returns [`FtpError::Io`] when the server cannot be reached and
    /// [`FtpError::Reply`] when it rejects the login.
    pub fn connect(host: &str) -> Result<Self, FtpError> {
        let host = host.trim();
        let address = resolve(host)?;
        let stream = TcpStream::connect_timeout(&address, CONNECT_TIMEOUT)?;
        stream.set_read_timeout(Some(TIMEOUT))?;
        stream.set_write_timeout(Some(TIMEOUT))?;
        let mut client = Self {
            reader: BufReader::new(stream.try_clone()?),
            writer: stream,
            peer: address.ip(),
            mlsd: true,
        };
        client.expect(&[220])?;
        let user = client.command("USER", "anonymous")?;
        if user.code == 331 {
            client.check("PASS", "", &[230, 202])?;
        } else if user.code != 230 {
            return Err(FtpError::Reply(user.code, user.text));
        }
        client.check("TYPE", "I", &[200])?;
        Ok(client)
    }

    /// Lists the directory `path` (absolute, `/`-separated).
    ///
    /// `.` and `..` are left out; the entries are sorted with directories
    /// first.
    ///
    /// # Errors
    ///
    /// Returns [`FtpError::Reply`] when the server refuses (for example the
    /// directory does not exist) and [`FtpError::Io`] on network errors.
    pub fn list(&mut self, path: &str) -> Result<Vec<FtpEntry>, FtpError> {
        let mut entries = Vec::new();
        if self.mlsd {
            match self.read_listing("MLSD", path) {
                Ok(text) => entries = text.lines().filter_map(parse_mlsd_line).collect(),
                Err(FtpError::Reply(code, _)) if (500..=504).contains(&code) => self.mlsd = false,
                Err(err) => return Err(err),
            }
        }
        if !self.mlsd {
            let text = self.read_listing("LIST", path)?;
            entries = text.lines().filter_map(parse_list_line).collect();
        }
        // A name is a single path component; anything else (such as the
        // listed directory reported with its full path) is not a child.
        entries.retain(|e| e.name != "." && e.name != ".." && !e.name.contains('/'));
        entries.sort_by(|a, b| {
            (a.kind != FtpEntryKind::Directory, &a.name)
                .cmp(&(b.kind != FtpEntryKind::Directory, &b.name))
        });
        Ok(entries)
    }

    /// Downloads the file `path` into `out`; `progress` receives the bytes
    /// received so far and may cancel. Returns the bytes received.
    ///
    /// # Errors
    ///
    /// Returns [`FtpError::Reply`] when the server refuses or reports a
    /// failed transfer, [`FtpError::Io`] on network or write errors and
    /// [`FtpError::Cancelled`] when `progress` cancelled (the connection must
    /// then be re-established).
    pub fn retrieve<W, F>(
        &mut self,
        path: &str,
        out: &mut W,
        mut progress: F,
    ) -> Result<u64, FtpError>
    where
        W: Write,
        F: FnMut(u64) -> ControlFlow<()>,
    {
        let mut data = self.open_data("RETR", path)?;
        let mut buffer = vec![0_u8; BUFFER];
        let mut received = 0_u64;
        loop {
            let n = data.read(&mut buffer)?;
            if n == 0 {
                break;
            }
            out.write_all(&buffer[..n])?;
            received += n as u64;
            if progress(received).is_break() {
                return Err(FtpError::Cancelled);
            }
        }
        drop(data);
        self.expect(&[226, 250])?;
        out.flush()?;
        Ok(received)
    }

    /// Logs out; errors are ignored because the connection closes anyway.
    pub fn quit(mut self) {
        let _ = self.command("QUIT", "");
    }

    /// Runs a listing command and returns the listing text.
    fn read_listing(&mut self, verb: &str, path: &str) -> Result<String, FtpError> {
        let mut data = self.open_data(verb, path)?;
        let mut bytes = Vec::new();
        data.read_to_end(&mut bytes)?;
        drop(data);
        self.expect(&[226, 250])?;
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }

    /// Opens a passive data connection and issues `verb argument` on it.
    fn open_data(&mut self, verb: &str, argument: &str) -> Result<TcpStream, FtpError> {
        let reply = self.command("PASV", "")?;
        if reply.code != 227 {
            return Err(FtpError::Reply(reply.code, reply.text));
        }
        // Connect to the control peer: the address in the reply may be a
        // private address of another interface.
        let port =
            parse_pasv_port(&reply.text).ok_or(FtpError::Protocol("unexpected PASV reply"))?;
        let data = TcpStream::connect_timeout(&SocketAddr::new(self.peer, port), TIMEOUT)?;
        data.set_read_timeout(Some(TIMEOUT))?;
        let reply = self.command(verb, argument)?;
        if !matches!(reply.code, 125 | 150) {
            return Err(FtpError::Reply(reply.code, reply.text));
        }
        Ok(data)
    }

    /// Sends a command and checks its reply code.
    fn check(&mut self, verb: &str, argument: &str, codes: &[u16]) -> Result<(), FtpError> {
        let reply = self.command(verb, argument)?;
        if codes.contains(&reply.code) {
            Ok(())
        } else {
            Err(FtpError::Reply(reply.code, reply.text))
        }
    }

    /// Sends a command and reads its reply. A command outside
    /// [`ALLOWED_COMMANDS`] is rejected without being sent.
    pub(crate) fn command(&mut self, verb: &str, argument: &str) -> Result<Reply, FtpError> {
        if !ALLOWED_COMMANDS.contains(&verb) {
            return Err(FtpError::Protocol("command not allowed in read-only mode"));
        }
        if argument.contains(['\r', '\n']) {
            return Err(FtpError::Protocol("line break in a path"));
        }
        let line = if argument.is_empty() {
            format!("{verb}\r\n")
        } else {
            format!("{verb} {argument}\r\n")
        };
        self.writer.write_all(line.as_bytes())?;
        self.read_reply()
    }

    /// Reads a reply and checks its code.
    fn expect(&mut self, codes: &[u16]) -> Result<Reply, FtpError> {
        let reply = self.read_reply()?;
        if codes.contains(&reply.code) {
            Ok(reply)
        } else {
            Err(FtpError::Reply(reply.code, reply.text))
        }
    }

    /// Reads one (possibly multi-line) reply.
    fn read_reply(&mut self) -> Result<Reply, FtpError> {
        let mut line = String::new();
        let mut first_code = None;
        loop {
            line.clear();
            if self.reader.read_line(&mut line)? == 0 {
                return Err(FtpError::Protocol("the server closed the connection"));
            }
            let trimmed = line.trim_end();
            let code = trimmed
                .get(..3)
                .and_then(|c| c.parse::<u16>().ok())
                .filter(|_| matches!(trimmed.as_bytes().get(3), None | Some(b' ' | b'-')));
            let Some(code) = code else {
                continue;
            };
            let first = *first_code.get_or_insert(code);
            // The reply ends at a line "ddd " with the same code.
            if code == first && trimmed.as_bytes().get(3) != Some(&b'-') {
                return Ok(Reply {
                    code,
                    text: trimmed.get(4..).unwrap_or("").to_owned(),
                });
            }
        }
    }
}

/// Resolves `host` with the default port when none is given.
fn resolve(host: &str) -> Result<SocketAddr, FtpError> {
    if let Ok(ip) = host.parse::<IpAddr>() {
        return Ok(SocketAddr::new(ip, DEFAULT_PORT));
    }
    let with_port = if host.contains(':') {
        host.to_owned()
    } else {
        format!("{host}:{DEFAULT_PORT}")
    };
    with_port
        .to_socket_addrs()?
        .next()
        .ok_or(FtpError::Protocol("cannot resolve the host"))
}

/// Extracts the port from `227 Entering Passive Mode (h1,h2,h3,h4,p1,p2)`.
pub(crate) fn parse_pasv_port(text: &str) -> Option<u16> {
    let start = text.find(|c: char| c.is_ascii_digit())?;
    let numbers: Vec<u8> = text[start..]
        .split(|c: char| !c.is_ascii_digit())
        .filter(|s| !s.is_empty())
        .take(6)
        .map(str::parse)
        .collect::<Result<_, _>>()
        .ok()?;
    let [_, _, _, _, high, low] = numbers.as_slice() else {
        return None;
    };
    Some(u16::from(*high) << 8 | u16::from(*low))
}
