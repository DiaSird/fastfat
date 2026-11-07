//! A minimal FTP server in a thread, imitating `ftpiiu` (MLSD replies that
//! name the listed folder with its full path, passive mode only).

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;

/// Files served by the fake server: (path, contents); directories are
/// implied by the paths.
pub(super) fn files() -> Vec<(&'static str, Vec<u8>)> {
    vec![
        ("/a.txt", b"hello".to_vec()),
        (
            "/storage_mlc/usr/title/0005000e/101c9400/meta/meta.xml",
            b"<xml/>".to_vec(),
        ),
        (
            "/storage_mlc/usr/title/0005000e/101c9400/content/big.bin",
            (0..300_000_u32).map(|i| (i % 251) as u8).collect(),
        ),
        ("/with space/name with space.dat", vec![7; 10]),
    ]
}

/// Lists the direct children of `dir`: (name, is_dir, size).
fn children(dir: &str) -> Option<Vec<(String, bool, usize)>> {
    let prefix = if dir.ends_with('/') {
        dir.to_owned()
    } else {
        format!("{dir}/")
    };
    let mut out: Vec<(String, bool, usize)> = Vec::new();
    let mut found = dir == "/";
    for (path, data) in files() {
        let Some(rest) = path.strip_prefix(&prefix) else {
            continue;
        };
        found = true;
        let (name, is_dir) = match rest.split_once('/') {
            Some((name, _)) => (name.to_owned(), true),
            None => (rest.to_owned(), false),
        };
        if !out.iter().any(|(n, _, _)| *n == name) {
            out.push((name, is_dir, data.len()));
        }
    }
    found.then_some(out)
}

/// Runs a fake server; returns its address and the log of received
/// commands.
pub(super) fn serve(mlsd: bool) -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap_or_else(|e| panic!("{e}"));
    let address = listener
        .local_addr()
        .unwrap_or_else(|e| panic!("{e}"))
        .to_string();
    let log = Arc::new(Mutex::new(Vec::new()));
    let log_server = Arc::clone(&log);
    thread::spawn(move || {
        // One thread per control connection, as parallel downloads open
        // several.
        for stream in listener.incoming().map_while(Result::ok) {
            let log = Arc::clone(&log_server);
            thread::spawn(move || {
                let _ = session(stream, mlsd, &log);
            });
        }
    });
    (address, log)
}

/// Handles one control connection.
fn session(stream: TcpStream, mlsd: bool, log: &Mutex<Vec<String>>) -> std::io::Result<()> {
    let mut writer = stream.try_clone()?;
    let mut reader = BufReader::new(stream);
    writer.write_all(b"220-Welcome\r\n220 ftpiiu fake\r\n")?;
    let mut passive: Option<TcpListener> = None;
    let mut line = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line)? == 0 {
            return Ok(());
        }
        let line = line.trim_end().to_owned();
        if let Ok(mut log) = log.lock() {
            log.push(line.clone());
        }
        let (verb, argument) = line.split_once(' ').unwrap_or((&line, ""));
        match verb {
            "USER" => writer.write_all(b"331 need password\r\n")?,
            "PASS" => writer.write_all(b"230 ok\r\n")?,
            "TYPE" | "NOOP" => writer.write_all(b"200 OK\r\n")?,
            "PASV" => {
                let listener = TcpListener::bind("127.0.0.1:0")?;
                let port = listener.local_addr()?.port();
                passive = Some(listener);
                // Report another address: the client must use the peer.
                let reply = format!(
                    "227 Entering Passive Mode (10,0,0,1,{},{}).\r\n",
                    port >> 8,
                    port & 0xFF
                );
                writer.write_all(reply.as_bytes())?;
            }
            "MLSD" if !mlsd => writer.write_all(b"502 not implemented\r\n")?,
            "MLSD" | "LIST" => {
                let Some(listener) = passive.take() else {
                    writer.write_all(b"425 no data connection\r\n")?;
                    continue;
                };
                let Some(entries) = children(argument) else {
                    writer.write_all(b"550 no such directory\r\n")?;
                    continue;
                };
                let (mut data, _) = listener.accept()?;
                writer.write_all(b"150 listing\r\n")?;
                for (name, is_dir, size) in entries {
                    let text = if verb == "MLSD" {
                        let kind = if is_dir { "dir" } else { "file" };
                        format!("Type={kind};Size={size};Modify=20240101000000;Perm=r; {name}\r\n")
                    } else {
                        let mode = if is_dir { 'd' } else { '-' };
                        format!("{mode}rwxr-xr-x 1 WiiU WiiU {size} Jan  1 12:00 {name}\r\n")
                    };
                    data.write_all(text.as_bytes())?;
                }
                if verb == "MLSD" {
                    // ftpiiu names the listed directory with its full path.
                    data.write_all(format!("Type=cdir;Size=0; {argument}\r\n").as_bytes())?;
                }
                drop(data);
                writer.write_all(b"226 done\r\n")?;
            }
            "RETR" => {
                let Some(listener) = passive.take() else {
                    writer.write_all(b"425 no data connection\r\n")?;
                    continue;
                };
                let Some((_, contents)) = files().into_iter().find(|(p, _)| *p == argument) else {
                    writer.write_all(b"550 no such file\r\n")?;
                    continue;
                };
                let (mut data, _) = listener.accept()?;
                writer.write_all(b"150 sending\r\n")?;
                let _ = data.write_all(&contents);
                drop(data);
                writer.write_all(b"226 done\r\n")?;
            }
            "QUIT" => {
                writer.write_all(b"221 bye\r\n")?;
                return Ok(());
            }
            _ => writer.write_all(b"502 not implemented\r\n")?,
        }
    }
}
