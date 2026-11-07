//! Tests of the FAT32 writer, including a mount check with `fatfs`.

use std::io::{Cursor, Read, Write};
use std::ops::ControlFlow;

use super::*;

const MIB: u64 = 1024 * 1024;

fn label(text: &str) -> VolumeLabel {
    VolumeLabel::parse(text).unwrap_or_else(|e| panic!("bad label: {e}"))
}

fn format_image(size: u64, options: &FormatOptions) -> (Vec<u8>, Layout) {
    let mut image = Cursor::new(vec![0xA5_u8; usize::try_from(size).unwrap_or(0)]);
    let geometry = Geometry::for_image(size);
    let layout = format(
        &mut image,
        &geometry,
        options,
        |_| ControlFlow::Continue(()),
    )
    .unwrap_or_else(|e| panic!("format failed: {e}"));
    (image.into_inner(), layout)
}

#[test]
fn auto_cluster_sizes() {
    assert_eq!(auto_cluster_size(64 * MIB), 512);
    assert_eq!(auto_cluster_size(300 * MIB), 4096);
    assert_eq!(auto_cluster_size(4 * 1024 * MIB), 4096);
    assert_eq!(auto_cluster_size(64 * 1024 * MIB), 32_768);
}

#[test]
fn label_encoding() {
    assert_eq!(VolumeLabel::parse("").ok(), Some(VolumeLabel::NO_NAME));
    assert_eq!(
        VolumeLabel::parse("usb").ok().map(|l| *l.as_bytes()),
        Some(*b"USB        ")
    );
    assert!(VolumeLabel::parse("TOO LONG LABEL").is_err());
    assert!(matches!(
        VolumeLabel::parse("A/B"),
        Err(FormatError::InvalidLabelChar('/'))
    ));
    assert!(VolumeLabel::parse("日本語").is_err());
}

#[test]
fn label_from_boot_sector() {
    let mut boot = [0_u8; 512];
    boot[71..82].copy_from_slice(b"WIIU       ");
    assert_eq!(VolumeLabel::from_boot_sector(&boot), label("wiiu"));
    boot[71..82].copy_from_slice(&[0xFF; 11]);
    assert_eq!(VolumeLabel::from_boot_sector(&boot), VolumeLabel::NO_NAME);
}

#[test]
fn rejects_small_volume() {
    let geometry = Geometry::for_image(16 * MIB);
    assert!(matches!(
        Layout::compute(&geometry, None),
        Err(FormatError::TooFewClusters(_))
    ));
}

#[test]
fn data_region_is_cluster_aligned() {
    let mut geometry = Geometry::for_image(300 * MIB);
    geometry.hidden_sectors = 63;
    let layout =
        Layout::compute(&geometry, Some(4096)).unwrap_or_else(|e| panic!("layout failed: {e}"));
    let start = geometry.hidden_sectors + layout.data_start_sector();
    assert_eq!(start % u64::from(layout.sectors_per_cluster), 0);
}

#[test]
fn boot_sector_fields() {
    let options = FormatOptions {
        label: label("Test"),
        volume_id: Some(0x1234_5678),
        ..FormatOptions::default()
    };
    let (image, layout) = format_image(64 * MIB, &options);
    assert_eq!(&image[510..512], &[0x55, 0xAA]);
    assert_eq!(&image[82..90], b"FAT32   ");
    assert_eq!(&image[71..82], b"TEST       ");
    assert_eq!(image[13], 1); // 512 byte clusters for 64 MiB
    let backup = 6 * 512;
    assert_eq!(&image[..512], &image[backup..backup + 512]);
    assert!(layout.cluster_count >= MIN_CLUSTERS);
}

#[test]
fn readable_by_fatfs() {
    let options = FormatOptions {
        label: label("FASTFAT"),
        ..FormatOptions::default()
    };
    let (image, layout) = format_image(64 * MIB, &options);

    let fs = fatfs::FileSystem::new(Cursor::new(image), fatfs::FsOptions::new())
        .unwrap_or_else(|e| panic!("fatfs could not mount the image: {e}"));
    assert_eq!(fs.fat_type(), fatfs::FatType::Fat32);
    assert_eq!(fs.volume_label().trim_end(), "FASTFAT");
    let stats = fs.stats().unwrap_or_else(|e| panic!("stats failed: {e}"));
    assert_eq!(stats.total_clusters(), layout.cluster_count);

    let root = fs.root_dir();
    {
        let mut file = root
            .create_file("hello.txt")
            .unwrap_or_else(|e| panic!("create failed: {e}"));
        file.write_all(b"hello fat32")
            .unwrap_or_else(|e| panic!("write failed: {e}"));
    }
    let mut text = String::new();
    root.open_file("hello.txt")
        .unwrap_or_else(|e| panic!("open failed: {e}"))
        .read_to_string(&mut text)
        .unwrap_or_else(|e| panic!("read failed: {e}"));
    assert_eq!(text, "hello fat32");
}

#[test]
fn cancel_stops_format() {
    let size = 64 * MIB;
    let mut image = Cursor::new(vec![0_u8; usize::try_from(size).unwrap_or(0)]);
    let options = FormatOptions {
        full: true,
        ..FormatOptions::default()
    };
    let result = format(&mut image, &Geometry::for_image(size), &options, |_| {
        ControlFlow::Break(())
    });
    assert!(matches!(result, Err(FormatError::Cancelled)));
    assert_eq!(&image.get_ref()[510..512], &[0, 0]);
}
