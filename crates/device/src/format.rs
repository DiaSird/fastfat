//! Formatting a volume in place or a whole disk.

use std::fs::File;
use std::io;
use std::ops::ControlFlow;

use windows_sys::Win32::System::Ioctl::{
    DISK_GEOMETRY, GET_LENGTH_INFORMATION, IOCTL_DISK_GET_DRIVE_GEOMETRY,
    IOCTL_DISK_GET_LENGTH_INFO, IOCTL_DISK_GET_PARTITION_INFO_EX, IOCTL_DISK_SET_PARTITION_INFO,
    IOCTL_DISK_UPDATE_PROPERTIES, PARTITION_INFORMATION_EX, PARTITION_STYLE_MBR,
    SET_PARTITION_INFORMATION,
};

use super::disks::{disk_size, open_disk};
use super::ioctl::{context, ioctl_none, ioctl_query, ioctl_set};
use super::volume::{Volume, lock_disk_volumes, open_query, single_disk, system_disk, volume_path};
use fastfat_fat32::{self as fat32, FormatError, FormatOptions, Geometry, Layout, Progress};
use fastfat_mbr::{self as mbr, PARTITION_TYPE_FAT32_LBA};

/// Formats the volume mounted at `letter` as FAT32.
///
/// The partition table is left untouched; for MBR disks the partition type
/// is updated to `0x0C` (FAT32 LBA) afterwards. The volume is locked and
/// dismounted for the duration of the format. This requires administrator
/// rights.
///
/// # Errors
///
/// Returns [`FormatError::Io`] when the volume cannot be opened, locked,
/// queried or written (for example when the program is not running as
/// administrator or files on the volume are still open), and any error
/// returned by [`fat32::format`].
pub fn format_drive<F>(
    letter: char,
    options: &FormatOptions,
    progress: F,
) -> Result<Layout, FormatError>
where
    F: FnMut(Progress) -> ControlFlow<()>,
{
    let mut volume = Volume::open(&volume_path(letter))?;
    let geometry = volume_geometry(&volume.file)?;
    let layout = fat32::format(&mut volume.file, &geometry, options, progress)?;
    volume.file.sync_all()?;
    set_fat32_partition_type(&volume.file);
    Ok(layout)
}

/// Re-initialises the whole disk holding `letter` as MBR with one FAT32
/// partition, as required by the Wii U.
///
/// Every volume on the disk is locked and dismounted first, so all its
/// partitions are erased. The disk must use 512 byte sectors; at most 2 TiB
/// are used. This requires administrator rights.
///
/// # Errors
///
/// Returns [`FormatError::Io`] when the disk holds the Windows system drive,
/// uses another sector size, a volume on it cannot be locked, or the disk
/// cannot be opened, queried or written, and any error returned by
/// [`mbr::format_disk`].
pub fn format_disk_mbr<F>(
    letter: char,
    options: &FormatOptions,
    progress: F,
) -> Result<Layout, FormatError>
where
    F: FnMut(Progress) -> ControlFlow<()>,
{
    let disk = open_query(&volume_path(letter))
        .and_then(|file| single_disk(&file))
        .map_err(|e| context(e, "cannot determine the physical disk of the volume"))?;
    if system_disk().is_ok_and(|system| system == disk) {
        return Err(io::Error::other("the disk holds the Windows system drive").into());
    }

    // Keep every volume of the disk locked until the new table is in place.
    let locks = lock_disk_volumes(disk)?;
    let mut physical = open_disk(disk, true)?;
    let (length, sector) = disk_size(&physical)?;
    if sector != mbr::SECTOR_SIZE {
        return Err(io::Error::other(format!(
            "the disk uses {sector} byte sectors; the Wii U compatible mode needs {} byte sectors",
            mbr::SECTOR_SIZE
        ))
        .into());
    }

    let result = mbr::format_disk(&mut physical, length / u64::from(sector), options, progress);
    // Whatever happened, the old partition table may be gone: flush and make
    // Windows re-read the table while the old volumes are still locked.
    let synced = physical.sync_all();
    let _ = ioctl_none(&physical, IOCTL_DISK_UPDATE_PROPERTIES);
    drop(physical);
    drop(locks);

    let layout = result?;
    synced?;
    Ok(layout)
}

/// Queries the sector size, size and partition offset of a volume.
fn volume_geometry(file: &File) -> io::Result<Geometry> {
    let disk: DISK_GEOMETRY = ioctl_query(file, IOCTL_DISK_GET_DRIVE_GEOMETRY)
        .map_err(|e| context(e, "cannot query the disk geometry"))?;
    let bytes_per_sector = disk.BytesPerSector;
    if bytes_per_sector == 0 {
        return Err(io::Error::other("the disk reported a sector size of 0"));
    }

    // Unpartitioned media may not report partition information; fall back
    // to the plain length with no offset.
    let partition = ioctl_query::<PARTITION_INFORMATION_EX>(file, IOCTL_DISK_GET_PARTITION_INFO_EX);
    let (offset, length) = if let Ok(part) = partition {
        (part.StartingOffset, part.PartitionLength)
    } else {
        let len: GET_LENGTH_INFORMATION = ioctl_query(file, IOCTL_DISK_GET_LENGTH_INFO)
            .map_err(|e| context(e, "cannot query the volume size"))?;
        (0, len.Length)
    };
    let offset = u64::try_from(offset).map_err(io::Error::other)?;
    let length = u64::try_from(length).map_err(io::Error::other)?;

    Ok(Geometry {
        bytes_per_sector,
        total_sectors: length / u64::from(bytes_per_sector),
        hidden_sectors: offset / u64::from(bytes_per_sector),
        sectors_per_track: u16::try_from(disk.SectorsPerTrack).unwrap_or(63),
        heads: u16::try_from(disk.TracksPerCylinder).unwrap_or(255),
    })
}

/// Marks the MBR partition of a volume as FAT32 (LBA).
///
/// This is best effort: GPT partitions, unpartitioned media ("super floppy")
/// and drivers that reject the request are left as is, because Windows
/// detects the file system from the boot sector anyway.
fn set_fat32_partition_type(file: &File) {
    let Ok(part) = ioctl_query::<PARTITION_INFORMATION_EX>(file, IOCTL_DISK_GET_PARTITION_INFO_EX)
    else {
        return;
    };
    if part.PartitionStyle != PARTITION_STYLE_MBR || part.PartitionNumber == 0 {
        return;
    }
    // SAFETY: the partition style is MBR, so `Mbr` is the active field.
    if unsafe { part.Anonymous.Mbr.PartitionType } == PARTITION_TYPE_FAT32_LBA {
        return;
    }
    let info = SET_PARTITION_INFORMATION {
        PartitionType: PARTITION_TYPE_FAT32_LBA,
    };
    if ioctl_set(file, IOCTL_DISK_SET_PARTITION_INFO, &info).is_ok() {
        let _ = ioctl_none(file, IOCTL_DISK_UPDATE_PROPERTIES);
    }
}
