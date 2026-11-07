//! Tests of the FTP client against an in-process fake server.

mod fake_server;

use std::fs;
use std::ops::ControlFlow;
use std::sync::Mutex;

use fake_server::{files, serve};

use crate::client::parse_pasv_port;
use crate::*;

#[test]
fn lists_and_downloads() {
    for mlsd in [true, false] {
        let (address, log) = serve(mlsd);
        let mut client = FtpClient::connect(&address).unwrap_or_else(|e| panic!("{e}"));

        let root = client.list("/").unwrap_or_else(|e| panic!("{e}"));
        let names: Vec<&str> = root.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["storage_mlc", "with space", "a.txt"]);
        assert_eq!(root[2].size, 5);

        let mut out = Vec::new();
        client
            .retrieve("/a.txt", &mut out, |_| ControlFlow::Continue(()))
            .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(out, b"hello");

        let spaced = client.list("/with space").unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(spaced[0].name, "name with space.dat");

        let updates = client
            .list("/storage_mlc/usr/title/0005000e")
            .unwrap_or_else(|e| panic!("{e}"));
        let dir =
            std::env::temp_dir().join(format!("fastfat-ftp-test-{}-{mlsd}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap_or_else(|e| panic!("{e}"));
        let plan = client
            .plan_download("/storage_mlc/usr/title/0005000e", &updates[..1], &dir)
            .unwrap_or_else(|e| panic!("{e}"));
        let planned: u64 = plan.files().iter().map(|f| f.size).sum();
        assert_eq!((plan.files().len(), planned), (2, 300_006));
        let stats = plan
            .download(&address, ExistingFiles::Fail, 1, |_| {
                ControlFlow::Continue(())
            })
            .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!((stats.files, stats.bytes), (2, 300_006));
        let big =
            fs::read(dir.join("101c9400").join("content").join("big.bin")).unwrap_or_default();
        assert_eq!(big.len(), 300_000);
        let _ = fs::remove_dir_all(&dir);

        assert!(matches!(
            client.list("/missing"),
            Err(FtpError::Reply(550, _))
        ));
        client.quit();

        // Only read-only commands were sent.
        let log = log.lock().map(|l| l.clone()).unwrap_or_default();
        assert!(log.iter().all(|line| {
            let verb = line.split(' ').next().unwrap_or("");
            ALLOWED_COMMANDS.contains(&verb)
        }));
    }
}

#[test]
fn downloads_in_parallel() {
    let (address, log) = serve(true);
    let mut client = FtpClient::connect(&address).unwrap_or_else(|e| panic!("{e}"));
    let parent = "/storage_mlc/usr/title/0005000e";
    let updates = client.list(parent).unwrap_or_else(|e| panic!("{e}"));
    let dir = std::env::temp_dir().join(format!("fastfat-ftp-parallel-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap_or_else(|e| panic!("{e}"));

    // (planned files, started, finished bytes)
    let seen = Mutex::new((Vec::new(), 0, 0));
    let stats = client
        .download_parallel(&address, parent, &updates, &dir, 4, |event| {
            if let Ok(mut seen) = seen.lock() {
                match event {
                    TransferEvent::Planned(files) => seen.0 = files.to_vec(),
                    TransferEvent::Started { .. } => seen.1 += 1,
                    TransferEvent::Finished { received, .. } => seen.2 += received,
                    TransferEvent::Progress { .. } => {}
                }
            }
            ControlFlow::Continue(())
        })
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!((stats.files, stats.bytes), (2, 300_006));
    let (planned, started, finished) = seen.into_inner().unwrap_or_default();
    assert_eq!(planned.len(), 2);
    assert_eq!(planned.iter().map(|f| f.size).sum::<u64>(), 300_006);
    assert!(
        planned
            .iter()
            .any(|f| f.remote.ends_with("/101c9400/content/big.bin"))
    );
    assert_eq!((started, finished), (2, 300_006));
    let big = fs::read(dir.join("101c9400").join("content").join("big.bin")).unwrap_or_default();
    assert_eq!(big, files()[2].1);

    // A second run refuses to overwrite and leaves the files intact.
    let again = client.download_parallel(&address, parent, &updates, &dir, 4, |_| {
        ControlFlow::Continue(())
    });
    assert!(matches!(again, Err(FtpError::Io(_))));
    assert_eq!(
        fs::read(dir.join("101c9400").join("meta").join("meta.xml")).unwrap_or_default(),
        b"<xml/>"
    );

    // Cancelling stops every connection and removes partial files.
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap_or_else(|e| panic!("{e}"));
    let cancelled = client.download_parallel(&address, parent, &updates, &dir, 2, |_| {
        ControlFlow::Break(())
    });
    assert!(matches!(cancelled, Err(FtpError::Cancelled)));
    assert!(
        !dir.join("101c9400")
            .join("content")
            .join("big.bin")
            .exists()
    );
    let _ = fs::remove_dir_all(&dir);

    let log = log.lock().map(|l| l.clone()).unwrap_or_default();
    assert!(
        log.iter()
            .all(|line| ALLOWED_COMMANDS.contains(&line.split(' ').next().unwrap_or("")))
    );
}

#[test]
fn detects_files_already_downloaded() {
    let (address, _) = serve(true);
    let mut client = FtpClient::connect(&address).unwrap_or_else(|e| panic!("{e}"));
    let parent = "/storage_mlc/usr/title/0005000e";
    let updates = client.list(parent).unwrap_or_else(|e| panic!("{e}"));
    let dir = std::env::temp_dir().join(format!("fastfat-ftp-existing-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap_or_else(|e| panic!("{e}"));
    let go_on = |_: TransferEvent<'_>| ControlFlow::Continue(());

    let plan = client
        .plan_download(parent, &updates, &dir)
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(plan.existing(), (0, 0));
    plan.download(&address, ExistingFiles::Fail, 1, go_on)
        .unwrap_or_else(|e| panic!("{e}"));

    // Everything is there with the right size: nothing is downloaded again.
    let plan = client
        .plan_download(parent, &updates, &dir)
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(plan.existing(), (2, 2));
    let stats = plan
        .download(&address, ExistingFiles::SkipComplete, 1, go_on)
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!((stats.files, stats.existing), (0, 2));

    // A truncated file counts as incomplete and is downloaded again.
    let big = dir.join("101c9400").join("content").join("big.bin");
    fs::write(&big, b"partial").unwrap_or_else(|e| panic!("{e}"));
    let plan = client
        .plan_download(parent, &updates, &dir)
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(plan.existing(), (2, 1));
    let stats = plan
        .download(&address, ExistingFiles::SkipComplete, 1, go_on)
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!((stats.files, stats.existing), (1, 1));
    assert_eq!(fs::read(&big).unwrap_or_default(), files()[2].1);

    // Overwrite downloads everything again.
    let stats = plan
        .download(&address, ExistingFiles::Overwrite, 2, go_on)
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!((stats.files, stats.existing), (2, 0));
    assert!(matches!(
        plan.download(&address, ExistingFiles::Fail, 1, go_on),
        Err(FtpError::Io(_))
    ));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn refuses_write_commands() {
    let (address, log) = serve(true);
    let mut client = FtpClient::connect(&address).unwrap_or_else(|e| panic!("{e}"));
    for verb in ["STOR", "DELE", "RMD", "MKD", "RNFR", "APPE", "SITE"] {
        assert!(matches!(
            client.command(verb, "/x"),
            Err(FtpError::Protocol(_))
        ));
    }
    assert!(matches!(
        client.command("RETR", "/a\r\nDELE /a"),
        Err(FtpError::Protocol(_))
    ));
    client.quit();
    let log = log.lock().map(|l| l.clone()).unwrap_or_default();
    assert!(
        !log.iter()
            .any(|l| l.starts_with("STOR") || l.starts_with("DELE"))
    );
}

#[test]
fn sanitizes_names() {
    assert_eq!(sanitize_name("a:b?c"), "a_b_c");
    assert_eq!(sanitize_name(".."), "_");
    assert_eq!(sanitize_name("name. "), "name");
}

#[test]
fn parses_listings() {
    let line = "drwxr-xr-x 1 WiiU WiiU 0 Jan  1 12:00 my dir";
    assert_eq!(
        parse_list_line(line),
        Some(FtpEntry {
            name: "my dir".to_owned(),
            kind: FtpEntryKind::Directory,
            size: 0
        })
    );
    let line = "-rw-r--r-- 1 WiiU WiiU 1234 Dec 31 2020 file.bin\r";
    assert_eq!(parse_list_line(line).map(|e| e.size), Some(1234));
    assert_eq!(parse_list_line("total 3"), None);

    let entry = parse_mlsd_line("Type=file;Size=42;Modify=20240101000000; a b.txt");
    assert_eq!(
        entry.map(|e| (e.name, e.size)),
        Some(("a b.txt".to_owned(), 42))
    );
    assert_eq!(
        parse_mlsd_line("type=dir;size=0; d").map(|e| e.kind),
        Some(FtpEntryKind::Directory)
    );

    assert_eq!(
        parse_pasv_port("Entering Passive Mode (192,168,0,10,195,80)."),
        Some(50000)
    );
    assert_eq!(join("/", "a"), "/a");
    assert_eq!(join("/a", "b"), "/a/b");
    assert_eq!(parent("/a/b"), "/a");
    assert_eq!(parent("/a"), "/");
}
