//! Enumeration and queries of physical disks.

use std::fmt::Write as _;
use std::fs::{File, OpenOptions};
use std::io;
use std::mem::size_of;
use std::os::windows::fs::OpenOptionsExt;
use std::ptr;

use windows_sys::Win32::Storage::FileSystem::{FILE_SHARE_READ, FILE_SHARE_WRITE};
use windows_sys::Win32::System::Ioctl::{
    DISK_GEOMETRY_EX, IOCTL_DISK_GET_DRIVE_GEOMETRY_EX, IOCTL_DISK_GET_DRIVE_LAYOUT_EX,
    IOCTL_STORAGE_QUERY_PROPERTY, PropertyStandardQuery, STORAGE_PROPERTY_QUERY,
    StorageDeviceProperty,
};

use super::PartitionStyle;
use super::drives::present_letters;
use super::ioctl::{context, device_io_control, ioctl_query};
use super::volume::{open_query, single_disk, style_from_raw, system_letter, volume_path};
use fastfat_units::write_size;

/// Highest disk number probed by [`list_disks`].
const MAX_DISKS: u32 = 64;

/// A physical disk (`\\.\PhysicalDriveN`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiskInfo {
    /// Disk number.
    pub number: u32,
    /// Vendor and product name reported by the device.
    pub model: String,
    /// Bus the disk is attached to.
    pub bus: &'static str,
    /// Size in bytes.
    pub size: u64,
    /// Logical sector size in bytes.
    pub bytes_per_sector: u32,
    /// Partition table style, if it could be read.
    pub partition_style: Option<PartitionStyle>,
    /// Drive letters of the volumes on the disk, e.g. `"E: F:"`.
    pub letters: String,
    /// Whether the disk holds the Windows system drive.
    pub is_system: bool,
    /// One-line description for display in a list.
    pub description: String,
}

/// Lists the physical disks of the computer.
///
/// No administrator rights are needed; only queries that do not require
/// access rights are issued.
#[must_use]
pub fn list_disks() -> Vec<DiskInfo> {
    // Map drive letters to disk numbers once.
    let letters: Vec<(char, u32)> = present_letters()
        .filter_map(|letter| {
            let file = open_query(&volume_path(letter)).ok()?;
            single_disk(&file).ok().map(|disk| (letter, disk))
        })
        .collect();
    let system = system_letter();

    (0..MAX_DISKS)
        .filter_map(|number| {
            let file = open_query(&physical_path(number)).ok()?;
            let geometry: DISK_GEOMETRY_EX =
                ioctl_query(&file, IOCTL_DISK_GET_DRIVE_GEOMETRY_EX).ok()?;
            let (model, bus) = storage_description(&file).unwrap_or_default();

            let mut on_disk = String::new();
            let mut is_system = false;
            for &(letter, _) in letters.iter().filter(|(_, disk)| *disk == number) {
                let _ = write!(
                    on_disk,
                    "{}{letter}:",
                    if on_disk.is_empty() { "" } else { " " }
                );
                is_system |= letter == system;
            }

            let mut info = DiskInfo {
                number,
                model,
                bus,
                size: u64::try_from(geometry.DiskSize).unwrap_or(0),
                bytes_per_sector: geometry.Geometry.BytesPerSector,
                partition_style: disk_partition_style(&file).ok(),
                letters: on_disk,
                is_system,
                description: String::new(),
            };
            info.description = describe_disk(&info);
            Some(info)
        })
        .collect()
}

/// Builds the one-line description of a disk.
fn describe_disk(info: &DiskInfo) -> String {
    let mut text = format!("Disk {}: ", info.number);
    text.push_str(if info.model.is_empty() {
        "(unknown)"
    } else {
        &info.model
    });
    text.push_str("  [");
    if !info.bus.is_empty() {
        let _ = write!(text, "{}, ", info.bus);
    }
    if let Some(style) = info.partition_style {
        let _ = write!(text, "{}, ", style.name());
    }
    write_size(&mut text, info.size);
    if !info.letters.is_empty() {
        let _ = write!(text, ", {}", info.letters);
    }
    if info.is_system {
        text.push_str(", SYSTEM");
    }
    text.push(']');
    text
}

/// Returns the device path of a physical disk.
pub(super) fn physical_path(number: u32) -> String {
    format!(r"\\.\PhysicalDrive{number}")
}

/// Opens a physical disk for raw access (administrator rights required).
pub(super) fn open_disk(number: u32, write: bool) -> io::Result<File> {
    let path = physical_path(number);
    OpenOptions::new()
        .read(true)
        .write(write)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .open(&path)
        .map_err(|e| context(e, &format!("cannot open {path} (run as administrator)")))
}

/// Returns the size in bytes (whole sectors) and the logical sector size of
/// a disk.
pub(super) fn disk_size(file: &File) -> io::Result<(u64, u32)> {
    let geometry: DISK_GEOMETRY_EX = ioctl_query(file, IOCTL_DISK_GET_DRIVE_GEOMETRY_EX)
        .map_err(|e| context(e, "cannot query the disk geometry"))?;
    let sector = geometry.Geometry.BytesPerSector;
    if sector == 0 {
        return Err(io::Error::other("the disk reported a sector size of 0"));
    }
    let size = u64::try_from(geometry.DiskSize).map_err(io::Error::other)?;
    Ok((size - size % u64::from(sector), sector))
}

/// Returns the partition table style of a physical disk.
fn disk_partition_style(file: &File) -> io::Result<PartitionStyle> {
    // DRIVE_LAYOUT_INFORMATION_EX has a variable number of entries; only the
    // leading style field is needed, but the buffer must hold all entries.
    let mut buffer = vec![0_u64; 2048];
    // SAFETY: `buffer` is writable for the length passed.
    unsafe {
        device_io_control(
            file,
            IOCTL_DISK_GET_DRIVE_LAYOUT_EX,
            ptr::null(),
            0,
            buffer.as_mut_ptr().cast(),
            buffer.len() * size_of::<u64>(),
        )?;
    }
    // The first field is the 32-bit PARTITION_STYLE (little endian).
    #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
    let style = buffer[0] as u32 as i32;
    Ok(style_from_raw(style))
}

/// Returns the model name and the bus type of a disk.
fn storage_description(file: &File) -> io::Result<(String, &'static str)> {
    let query = STORAGE_PROPERTY_QUERY {
        PropertyId: StorageDeviceProperty,
        QueryType: PropertyStandardQuery,
        AdditionalParameters: [0],
    };
    let mut buffer = vec![0_u8; 1024];
    // SAFETY: `query` and `buffer` are valid for the lengths passed.
    unsafe {
        device_io_control(
            file,
            IOCTL_STORAGE_QUERY_PROPERTY,
            ptr::from_ref(&query).cast(),
            size_of::<STORAGE_PROPERTY_QUERY>(),
            buffer.as_mut_ptr().cast(),
            buffer.len(),
        )?;
    }

    // STORAGE_DEVICE_DESCRIPTOR: string offsets at 12 (vendor) and
    // 16 (product), bus type at 28.
    let field = |offset: usize| {
        buffer
            .get(offset..offset + 4)
            .and_then(|b| b.try_into().ok())
            .map_or(0, u32::from_le_bytes)
    };
    let string = |offset: u32| {
        let start = usize::try_from(offset).unwrap_or(0);
        let bytes = if start == 0 {
            &[][..]
        } else {
            buffer.get(start..).unwrap_or(&[])
        };
        let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
        String::from_utf8_lossy(&bytes[..end]).trim().to_owned()
    };
    let vendor = string(field(12));
    let product = string(field(16));
    let model = match (vendor.is_empty(), product.is_empty()) {
        (true, _) => product,
        (false, true) => vendor,
        (false, false) => format!("{vendor} {product}"),
    };
    Ok((model, bus_name(field(28))))
}

/// Returns the display name of a `STORAGE_BUS_TYPE` value.
const fn bus_name(bus: u32) -> &'static str {
    match bus {
        1 => "SCSI",
        3 => "ATA",
        7 => "USB",
        10 => "SAS",
        11 => "SATA",
        12 => "SD",
        13 => "MMC",
        17 => "NVMe",
        _ => "",
    }
}
