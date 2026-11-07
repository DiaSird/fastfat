//! Disk signature replacement for MBR clones.

use fastfat_mbr as mbr;

/// Offset of the disk signature inside an MBR.
const MBR_SIGNATURE: usize = 440;

/// Replaces the disk signature of an MBR disk in its first sector.
///
/// Windows takes a disk offline when two attached disks share a signature, so
/// a clone normally gets a fresh one. GPT disks (protective MBR) and sectors
/// without a boot signature are left untouched.
///
/// Returns `true` when the signature was replaced.
pub fn replace_mbr_signature(sector: &mut [u8], signature: u32) -> bool {
    if mbr::parse_partitions(sector).is_none() {
        return false;
    }
    sector[MBR_SIGNATURE..MBR_SIGNATURE + 4].copy_from_slice(&signature.to_le_bytes());
    true
}
