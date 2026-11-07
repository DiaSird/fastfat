//! Writing the FAT32 on-disk structures.

use std::io::{self, Seek, SeekFrom, Write};
use std::ops::ControlFlow;
use std::time::{SystemTime, UNIX_EPOCH};

use super::{FormatError, Geometry, Layout, Progress, VolumeLabel};

/// Sector index of the FSInfo structure inside the reserved region.
const FSINFO_SECTOR: u16 = 1;

/// Sector index of the backup boot sector inside the reserved region.
const BACKUP_BOOT_SECTOR: u16 = 6;

/// Number of sectors of the boot record (boot sector, FSInfo, spare sector).
const BOOT_RECORD_SECTORS: usize = 3;

/// First data cluster, used for the root directory.
const ROOT_CLUSTER: u32 = 2;

/// Media descriptor for fixed (non-floppy) media.
const MEDIA_DESCRIPTOR: u8 = 0xF8;

/// Size of the zero buffer used for bulk writes (4 MiB).
const ZERO_CHUNK: usize = 4 * 1024 * 1024;

/// User selectable options for [`format`].
///
/// The type is `Copy`, so it can be handed to a worker thread without
/// allocating.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FormatOptions {
    /// Cluster size in bytes, or `None` to pick one with
    /// [`super::auto_cluster_size`].
    pub cluster_size: Option<u32>,
    /// Volume label.
    pub label: VolumeLabel,
    /// When `true`, every data cluster is overwritten with zeros
    /// (slow "full" format). Otherwise only the metadata is written.
    pub full: bool,
    /// Volume serial number, or `None` to derive one from the current time.
    pub volume_id: Option<u32>,
}

/// Derives a volume serial number from the current time.
#[must_use]
pub fn default_volume_id() -> u32 {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    // Truncation is intended: only the low bits are mixed into the serial.
    #[allow(clippy::cast_possible_truncation)]
    let secs = now.as_secs() as u32;
    secs.rotate_left(16) ^ now.subsec_nanos()
}

/// Formats `device` as FAT32.
///
/// The device may be positioned anywhere; all writes use absolute offsets
/// relative to the start of the volume. The boot sector is written last so
/// that an interrupted or cancelled format never leaves a volume that looks
/// valid.
///
/// `progress` is called after each chunk is written. Returning
/// [`ControlFlow::Break`] cancels the operation.
///
/// # Errors
///
/// Returns an error when the layout cannot be computed (see
/// [`Layout::compute`]), a write fails, or the callback cancelled the
/// operation.
pub fn format<D, F>(
    device: &mut D,
    geometry: &Geometry,
    options: &FormatOptions,
    mut progress: F,
) -> Result<Layout, FormatError>
where
    D: Write + Seek + ?Sized,
    F: FnMut(Progress) -> ControlFlow<()>,
{
    let layout = Layout::compute(geometry, options.cluster_size)?;
    let label = options.label.as_bytes();
    let volume_id = options.volume_id.unwrap_or_else(default_volume_id);

    let system_end = layout.data_start_sector() + u64::from(layout.sectors_per_cluster);
    let clear_end = if options.full {
        u64::from(layout.total_sectors)
    } else {
        system_end
    };
    let total = layout.offset(clear_end);
    let mut tracker = Tracker {
        progress: Progress { written: 0, total },
        callback: &mut progress,
    };

    // 1. Clear the reserved region, both FATs and the root directory cluster
    //    (and the whole data region for a full format).
    write_zeros(device, 0, total, &mut tracker)?;

    // 2. First sector of each FAT: media descriptor, clean-shutdown flags
    //    and the end-of-chain marker for the root directory.
    let mut fat_head = vec![0_u8; layout.sector_len()];
    put_u32(&mut fat_head, 0, 0x0FFF_FF00 | u32::from(MEDIA_DESCRIPTOR));
    put_u32(&mut fat_head, 4, 0x0FFF_FFFF);
    put_u32(&mut fat_head, 8, 0x0FFF_FFFF);
    for fat in 0..u64::from(layout.fat_count) {
        let sector = u64::from(layout.reserved_sectors) + fat * u64::from(layout.fat_sectors);
        write_at(device, layout.offset(sector), &fat_head)?;
    }

    // 3. Volume label entry in the root directory.
    if options.label != VolumeLabel::NO_NAME {
        let mut root = vec![0_u8; layout.sector_len()];
        root[..11].copy_from_slice(label);
        root[11] = 0x08; // ATTR_VOLUME_ID
        write_at(device, layout.offset(layout.data_start_sector()), &root)?;
    }

    // 4. Boot record: backup copy first, primary copy last.
    let boot_record = build_boot_record(&layout, geometry, label, volume_id);
    write_at(
        device,
        layout.offset(u64::from(BACKUP_BOOT_SECTOR)),
        &boot_record,
    )?;
    write_at(device, 0, &boot_record)?;

    device.flush()?;
    Ok(layout)
}

/// Bookkeeping for progress reporting during [`format`].
struct Tracker<'a, F> {
    progress: Progress,
    callback: &'a mut F,
}

impl<F: FnMut(Progress) -> ControlFlow<()>> Tracker<'_, F> {
    /// Records `bytes` as written and notifies the callback.
    fn advance(&mut self, bytes: u64) -> Result<(), FormatError> {
        self.progress.written += bytes;
        match (self.callback)(self.progress) {
            ControlFlow::Continue(()) => Ok(()),
            ControlFlow::Break(()) => Err(FormatError::Cancelled),
        }
    }
}

/// Writes `buf` at the absolute byte `offset`.
fn write_at<D: Write + Seek + ?Sized>(device: &mut D, offset: u64, buf: &[u8]) -> io::Result<()> {
    device.seek(SeekFrom::Start(offset))?;
    device.write_all(buf)
}

/// Writes `len` zero bytes starting at the absolute byte `offset`.
fn write_zeros<D, F>(
    device: &mut D,
    offset: u64,
    len: u64,
    tracker: &mut Tracker<'_, F>,
) -> Result<(), FormatError>
where
    D: Write + Seek + ?Sized,
    F: FnMut(Progress) -> ControlFlow<()>,
{
    let chunk = usize::try_from(len).map_or(ZERO_CHUNK, |len| len.min(ZERO_CHUNK));
    let zeros = vec![0_u8; chunk];
    device.seek(SeekFrom::Start(offset))?;
    let mut remaining = len;
    while remaining > 0 {
        let n = usize::try_from(remaining).map_or(zeros.len(), |r| r.min(zeros.len()));
        device.write_all(&zeros[..n])?;
        remaining -= n as u64;
        tracker.advance(n as u64)?;
    }
    Ok(())
}

/// Stores `value` as little endian `u16` at `offset`.
fn put_u16(buf: &mut [u8], offset: usize, value: u16) {
    buf[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
}

/// Stores `value` as little endian `u32` at `offset`.
fn put_u32(buf: &mut [u8], offset: usize, value: u32) {
    buf[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

/// Builds the three sector boot record: boot sector, FSInfo and a spare
/// sector carrying only the boot signature.
fn build_boot_record(
    layout: &Layout,
    geometry: &Geometry,
    label: &[u8; 11],
    volume_id: u32,
) -> Vec<u8> {
    let sector = layout.sector_len();
    let mut record = vec![0_u8; sector * BOOT_RECORD_SECTORS];
    let (boot, rest) = record.split_at_mut(sector);
    let (fsinfo, spare) = rest.split_at_mut(sector);

    write_boot_sector(boot, layout, geometry, label, volume_id);
    write_fsinfo(fsinfo, layout);
    spare[510] = 0x55;
    spare[511] = 0xAA;
    record
}

/// Fills the BIOS parameter block of the FAT32 boot sector.
// The narrowing casts below are bounded by `Layout::compute`.
#[allow(clippy::cast_possible_truncation)]
fn write_boot_sector(
    boot: &mut [u8],
    layout: &Layout,
    geometry: &Geometry,
    label: &[u8; 11],
    volume_id: u32,
) {
    // Jump instruction and OEM name.
    boot[0..3].copy_from_slice(&[0xEB, 0x58, 0x90]);
    boot[3..11].copy_from_slice(b"MSWIN4.1");

    // Common BIOS parameter block.
    put_u16(boot, 11, layout.bytes_per_sector as u16);
    boot[13] = layout.sectors_per_cluster as u8;
    put_u16(boot, 14, layout.reserved_sectors as u16);
    boot[16] = layout.fat_count as u8;
    put_u16(boot, 17, 0); // root entry count: always 0 on FAT32
    put_u16(boot, 19, 0); // 16-bit total sectors: unused on FAT32
    boot[21] = MEDIA_DESCRIPTOR;
    put_u16(boot, 22, 0); // 16-bit FAT size: unused on FAT32
    put_u16(boot, 24, geometry.sectors_per_track);
    put_u16(boot, 26, geometry.heads);
    put_u32(boot, 28, u32::try_from(layout.hidden_sectors).unwrap_or(0));
    put_u32(boot, 32, layout.total_sectors);

    // FAT32 extended BIOS parameter block.
    put_u32(boot, 36, layout.fat_sectors);
    put_u16(boot, 40, 0); // flags: FAT mirroring enabled
    put_u16(boot, 42, 0); // file system version 0.0
    put_u32(boot, 44, ROOT_CLUSTER);
    put_u16(boot, 48, FSINFO_SECTOR);
    put_u16(boot, 50, BACKUP_BOOT_SECTOR);
    boot[64] = 0x80; // BIOS drive number
    boot[66] = 0x29; // extended boot signature
    put_u32(boot, 67, volume_id);
    boot[71..82].copy_from_slice(label);
    boot[82..90].copy_from_slice(b"FAT32   ");

    // Minimal boot code: ask the BIOS to try the next device (INT 18h) and
    // halt if it returns.
    boot[90..94].copy_from_slice(&[0xCD, 0x18, 0xEB, 0xFE]);

    boot[510] = 0x55;
    boot[511] = 0xAA;
}

/// Fills the FSInfo sector.
fn write_fsinfo(fsinfo: &mut [u8], layout: &Layout) {
    put_u32(fsinfo, 0, 0x4161_5252); // lead signature "RRaA"
    put_u32(fsinfo, 484, 0x6141_7272); // structure signature "rrAa"
    put_u32(fsinfo, 488, layout.cluster_count - 1); // free clusters (root uses one)
    put_u32(fsinfo, 492, ROOT_CLUSTER + 1); // next free cluster hint
    put_u32(fsinfo, 508, 0xAA55_0000); // trail signature
}
