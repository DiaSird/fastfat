//! Tests of the streaming copy, the tree copy and the signature replacement.

use std::fs;
use std::io::Cursor;
use std::ops::ControlFlow;
use std::path::Path;

use super::*;
use fastfat_fat32::FormatError;

#[test]
fn copies_across_chunks() {
    let len = CHUNK_SIZE * 2 + 4096;
    let data: Vec<u8> = (0..len).map(|i| (i % 251) as u8).collect();
    let mut source = Cursor::new(data);
    let mut destination = Cursor::new(vec![0xFF_u8; len + 512]);
    let mut calls = 0;
    copy(&mut source, &mut destination, len as u64, |_| {
        calls += 1;
        ControlFlow::Continue(())
    })
    .unwrap_or_else(|e| panic!("copy failed: {e}"));

    assert_eq!(calls, 3);
    assert_eq!(&destination.get_ref()[..len], source.get_ref().as_slice());
    // Bytes past `len` are untouched.
    assert_eq!(&destination.get_ref()[len..], &[0xFF; 512]);
}

#[test]
fn cancel_stops_copy() {
    let len = CHUNK_SIZE * 3;
    let mut source = Cursor::new(vec![1_u8; len]);
    let mut destination = Cursor::new(vec![0_u8; len]);
    let result = copy(&mut source, &mut destination, len as u64, |_| {
        ControlFlow::Break(())
    });
    assert!(matches!(result, Err(FormatError::Cancelled)));
    assert_eq!(destination.get_ref()[len - 1], 0);
}

#[test]
fn short_source_is_an_error() {
    let mut source = Cursor::new(vec![1_u8; 4096]);
    let mut destination = Cursor::new(vec![0_u8; 8192]);
    let result = copy(&mut source, &mut destination, 8192, |_| {
        ControlFlow::Continue(())
    });
    assert!(matches!(result, Err(FormatError::Io(_))));
}

#[test]
fn copies_a_tree() {
    let base = std::env::temp_dir().join(format!("fastfat-tree-{}", std::process::id()));
    let source = base.join("src");
    let destination = base.join("dst");
    let _ = fs::remove_dir_all(&base);
    let write = |path: &Path, len: usize| {
        fs::write(path, vec![7_u8; len]).unwrap_or_else(|e| panic!("write failed: {e}"));
    };
    fs::create_dir_all(source.join("wudump/game")).unwrap_or_else(|e| panic!("mkdir failed: {e}"));
    fs::create_dir_all(source.join("System Volume Information"))
        .unwrap_or_else(|e| panic!("mkdir failed: {e}"));
    write(&source.join("small.txt"), 10);
    write(&source.join("wudump/game/big.app"), CHUNK_SIZE + 100);
    write(&source.join("System Volume Information/x"), 5);

    let skip = ["system volume information"];
    let total = tree_size(&source, &skip).unwrap_or_else(|e| panic!("tree_size failed: {e}"));
    assert_eq!(total, (CHUNK_SIZE + 110) as u64);

    let mut copied = 0;
    copy_tree(&source, &destination, &skip, |n| {
        copied += n;
        ControlFlow::Continue(())
    })
    .unwrap_or_else(|e| panic!("copy_tree failed: {e}"));

    assert_eq!(copied, total);
    let big = fs::metadata(destination.join("wudump/game/big.app")).map_or(0, |m| m.len());
    assert_eq!(big, (CHUNK_SIZE + 100) as u64);
    assert!(!destination.join("System Volume Information").exists());
    let _ = fs::remove_dir_all(&base);
}

#[test]
fn signature_replacement() {
    let mut mbr = fastfat_mbr::build_mbr(2048, 100_000, 0x1111_1111);
    assert!(replace_mbr_signature(&mut mbr, 0x2222_2222));
    assert_eq!(&mbr[440..444], &0x2222_2222_u32.to_le_bytes());

    mbr[450] = 0xEE;
    assert!(!replace_mbr_signature(&mut mbr, 0x3333_3333));
    assert!(!replace_mbr_signature(&mut [0_u8; 512], 1));
}
