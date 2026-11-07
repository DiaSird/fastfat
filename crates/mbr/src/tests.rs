//! Tests of the MBR table, the expansion plan and whole-disk formatting.

use std::io::Cursor;
use std::ops::ControlFlow;

use super::table::chs;
use super::*;
use fastfat_fat32::{FormatError, FormatOptions};

const MIB: u64 = 1024 * 1024;

/// Partition table of the USB stick: FAT32 10.4 GB + hidden 47.3 GB.
fn usb_layout() -> [PartitionEntry; 4] {
    [
        PartitionEntry {
            status: 0,
            kind: 0x0C,
            start: 8192,
            sectors: 21_880_832,
        },
        PartitionEntry {
            status: 0,
            kind: 0x17,
            start: 21_889_024,
            sectors: 99_256_320,
        },
        PartitionEntry::default(),
        PartitionEntry::default(),
    ]
}

#[test]
fn partition_table_round_trip() {
    let entries = usb_layout();
    let mut sector = [0_u8; 512];
    write_partitions(&mut sector, &entries);
    assert_eq!(parse_partitions(&sector), Some(entries));

    sector[450] = 0xEE;
    assert_eq!(parse_partitions(&sector), None);
}

#[test]
fn expansion_moves_hidden_partition_to_the_end() {
    let disk = 1_953_525_168_u64; // 1 TB
    let (fat, planned) =
        plan_expansion(&usb_layout(), disk).unwrap_or_else(|e| panic!("plan failed: {e}"));
    assert_eq!(fat, 0);

    let hidden = planned[1];
    assert_eq!(hidden.sectors, usb_layout()[1].sectors);
    assert_eq!(hidden.start % PARTITION_START, 0);
    assert!(hidden.end() <= disk);
    assert!(disk - hidden.end() < u64::from(PARTITION_START));

    let fat32 = planned[0];
    assert_eq!(fat32.start, 8192);
    assert_eq!(fat32.end(), u64::from(hidden.start));
}

#[test]
fn expansion_without_partitions_behind() {
    let mut entries = usb_layout();
    entries[1] = PartitionEntry::default();
    let (_, planned) =
        plan_expansion(&entries, 1_000_000_000).unwrap_or_else(|e| panic!("plan failed: {e}"));
    assert_eq!(planned[0].end(), 1_000_000_000);
}

#[test]
fn expansion_rejects_smaller_disk() {
    let result = plan_expansion(&usb_layout(), 100_000_000);
    assert!(matches!(
        result,
        Err(FormatError::UnsupportedPartitionTable(_))
    ));
}

#[test]
fn fresh_storage_layout() {
    let disk = 1_953_525_168_u64; // 1 TB
    let storage = 200 * 1024 * 1024 * 2; // 200 GiB in sectors
    let (fat, planned) = plan_fresh_storage(&usb_layout(), disk, storage)
        .unwrap_or_else(|e| panic!("plan failed: {e}"));
    assert_eq!(fat, 0);

    let fat32 = planned[0];
    let wii_u = planned[1];
    assert_eq!(fat32.kind, PARTITION_TYPE_FAT32_LBA);
    assert_eq!(fat32.start, 8192);
    assert_eq!(fat32.end(), u64::from(wii_u.start));
    assert_eq!(wii_u.kind, PARTITION_TYPE_WII_U_STORAGE);
    assert_eq!(wii_u.start % PARTITION_START, 0);
    assert!(u64::from(wii_u.sectors) >= storage);
    assert_eq!(wii_u.end(), disk);
    assert!(planned[2].is_empty() && planned[3].is_empty());

    let (_, whole) =
        plan_fresh_storage(&usb_layout(), disk, 0).unwrap_or_else(|e| panic!("plan failed: {e}"));
    assert_eq!(whole[0].end(), disk);
    assert!(whole[1].is_empty());

    assert!(plan_fresh_storage(&usb_layout(), disk, disk).is_err());
}

#[test]
fn chs_encoding() {
    assert_eq!(chs(2048), [32, 33, 0]);
    assert_eq!(chs(u32::MAX), [0xFE, 0xFF, 0xFF]);
}

#[test]
fn partition_is_capped_at_mbr_limit() {
    assert_eq!(
        partition_sectors(8 * 1024 * 1024 * 1024),
        u64::from(u32::MAX - PARTITION_START)
    );
    assert_eq!(partition_sectors(4096), 2048);
}

#[test]
fn replaces_gpt_with_mbr() {
    let size = 80 * MIB;
    let sectors = size / 512;
    let mut disk = Cursor::new(vec![0_u8; usize::try_from(size).unwrap_or(0)]);
    // Fake protective MBR and GPT headers that must disappear.
    disk.get_mut()[450] = 0xEE;
    disk.get_mut()[512..520].copy_from_slice(b"EFI PART");
    let backup = usize::try_from(size - 512).unwrap_or(0);
    disk.get_mut()[backup..backup + 8].copy_from_slice(b"EFI PART");

    let options = FormatOptions {
        volume_id: Some(0xCAFE_F00D),
        ..FormatOptions::default()
    };
    let layout = format_disk(&mut disk, sectors, &options, |_| ControlFlow::Continue(()))
        .unwrap_or_else(|e| panic!("format_disk failed: {e}"));

    let image = disk.get_ref();
    assert_eq!(&image[510..512], &[0x55, 0xAA]);
    assert_eq!(image[450], PARTITION_TYPE_FAT32_LBA);
    assert_eq!(&image[454..458], &PARTITION_START.to_le_bytes());
    assert_eq!(&image[458..462], &layout.total_sectors.to_le_bytes());
    assert_eq!(&image[440..444], &0xCAFE_F00D_u32.to_le_bytes());
    assert_eq!(&image[512..520], &[0; 8]);
    assert_eq!(&image[backup..backup + 8], &[0; 8]);
    // Hidden sectors in the boot sector must match the partition start.
    let boot = usize::try_from(u64::from(PARTITION_START) * 512).unwrap_or(0);
    assert_eq!(&image[boot + 28..boot + 32], &PARTITION_START.to_le_bytes());

    // The partition must be a valid FAT32 file system.
    let view = PartitionView::new(&mut disk, u64::from(PARTITION_START) * 512);
    let fs = fatfs::FileSystem::new(view, fatfs::FsOptions::new())
        .unwrap_or_else(|e| panic!("fatfs could not mount the partition: {e}"));
    assert_eq!(fs.fat_type(), fatfs::FatType::Fat32);
}
