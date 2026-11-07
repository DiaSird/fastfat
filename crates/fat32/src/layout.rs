//! FAT32 size computations: geometry of the target and resulting layout.

use super::{FormatError, MAX_CLUSTER_SIZE, MAX_CLUSTERS, MIN_CLUSTERS};

/// Number of reserved sectors before alignment padding is added.
const BASE_RESERVED_SECTORS: u32 = 32;

/// Number of FAT copies written to the volume.
const FAT_COUNT: u32 = 2;

/// Physical description of the volume that is going to be formatted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Geometry {
    /// Logical sector size in bytes.
    pub bytes_per_sector: u32,
    /// Size of the volume in sectors.
    pub total_sectors: u64,
    /// Number of sectors that precede the volume on the disk
    /// (the partition start LBA).
    pub hidden_sectors: u64,
    /// Sectors per track reported by the disk (only informational).
    pub sectors_per_track: u16,
    /// Number of heads reported by the disk (only informational).
    pub heads: u16,
}

impl Geometry {
    /// Creates a geometry for a stand-alone image of `size_bytes` bytes with
    /// 512 byte sectors and no partition offset.
    #[must_use]
    pub const fn for_image(size_bytes: u64) -> Self {
        Self {
            bytes_per_sector: 512,
            total_sectors: size_bytes / 512,
            hidden_sectors: 0,
            sectors_per_track: 63,
            heads: 255,
        }
    }

    /// Returns the size of the volume in bytes.
    #[must_use]
    pub const fn size_bytes(&self) -> u64 {
        self.total_sectors * self.bytes_per_sector as u64
    }
}

/// Computed FAT32 layout of a volume.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Layout {
    /// Logical sector size in bytes.
    pub bytes_per_sector: u32,
    /// Sectors per cluster.
    pub sectors_per_cluster: u32,
    /// Sectors in the reserved region (boot record and padding).
    pub reserved_sectors: u32,
    /// Number of FAT copies.
    pub fat_count: u32,
    /// Size of one FAT copy in sectors.
    pub fat_sectors: u32,
    /// Size of the volume in sectors.
    pub total_sectors: u32,
    /// Number of data clusters.
    pub cluster_count: u32,
    /// Number of sectors preceding the volume on the disk.
    pub hidden_sectors: u64,
}

impl Layout {
    /// Computes a FAT32 layout for `geometry`.
    ///
    /// When `cluster_size` is `None` the size is chosen by
    /// [`auto_cluster_size`], falling back to smaller clusters when the volume
    /// would have too few. The reserved region is padded so that the data
    /// region starts on a cluster boundary relative to the start of the disk.
    ///
    /// # Errors
    ///
    /// Returns an error when the sector or cluster size is unsupported, or when
    /// the volume is too small or too large for FAT32 with the chosen cluster
    /// size.
    pub fn compute(geometry: &Geometry, cluster_size: Option<u32>) -> Result<Self, FormatError> {
        if let Some(size) = cluster_size {
            return Self::compute_with(geometry, size);
        }
        let mut size = auto_cluster_size(geometry.size_bytes());
        loop {
            match Self::compute_with(geometry, size) {
                Err(FormatError::TooFewClusters(_)) if size > geometry.bytes_per_sector => {
                    size /= 2;
                }
                result => return result,
            }
        }
    }

    /// Computes a layout for an explicit cluster size in bytes.
    fn compute_with(geometry: &Geometry, cluster_size: u32) -> Result<Self, FormatError> {
        let bps = geometry.bytes_per_sector;
        if !matches!(bps, 512 | 1024 | 2048 | 4096) {
            return Err(FormatError::UnsupportedSectorSize(bps));
        }
        let total_sectors =
            u32::try_from(geometry.total_sectors).map_err(|_| FormatError::VolumeTooLarge)?;

        if !cluster_size.is_power_of_two() || cluster_size < bps || cluster_size > MAX_CLUSTER_SIZE
        {
            return Err(FormatError::InvalidClusterSize(cluster_size));
        }
        let spc = cluster_size / bps;
        if total_sectors <= BASE_RESERVED_SECTORS + spc {
            return Err(FormatError::TooFewClusters(0));
        }

        // FAT size in sectors, from the formula in the Microsoft specification
        // adapted for FAT32 (4 bytes per entry). The extra sector covers the
        // two reserved FAT entries.
        let usable = u64::from(total_sectors - BASE_RESERVED_SECTORS);
        let numerator = 4 * usable;
        let denominator = u64::from(cluster_size) + 4 * u64::from(FAT_COUNT);
        let fat_sectors = u32::try_from(numerator.div_ceil(denominator) + 1)
            .map_err(|_| FormatError::VolumeTooLarge)?;

        // Pad the reserved region so that the first data sector is aligned to
        // the cluster size on the physical disk.
        let unaligned_data_start =
            geometry.hidden_sectors + u64::from(BASE_RESERVED_SECTORS + FAT_COUNT * fat_sectors);
        let misalignment = unaligned_data_start % u64::from(spc);
        let padding = if misalignment == 0 {
            0
        } else {
            u32::try_from(u64::from(spc) - misalignment).map_err(|_| FormatError::VolumeTooLarge)?
        };
        let reserved_sectors = BASE_RESERVED_SECTORS + padding;

        let system_sectors = u64::from(reserved_sectors) + u64::from(FAT_COUNT * fat_sectors);
        let data_sectors = u64::from(total_sectors).saturating_sub(system_sectors);
        let cluster_count = u32::try_from(data_sectors / u64::from(spc)).unwrap_or(u32::MAX);

        if cluster_count < MIN_CLUSTERS {
            return Err(FormatError::TooFewClusters(cluster_count));
        }
        if cluster_count > MAX_CLUSTERS {
            return Err(FormatError::TooManyClusters(cluster_count));
        }
        // Each FAT must hold an entry for every cluster plus the two reserved entries.
        debug_assert!(
            (u64::from(cluster_count) + 2) * 4 <= u64::from(fat_sectors) * u64::from(bps)
        );

        Ok(Self {
            bytes_per_sector: bps,
            sectors_per_cluster: spc,
            reserved_sectors,
            fat_count: FAT_COUNT,
            fat_sectors,
            total_sectors,
            cluster_count,
            hidden_sectors: geometry.hidden_sectors,
        })
    }

    /// Returns the cluster size in bytes.
    #[must_use]
    pub const fn cluster_size(&self) -> u32 {
        self.sectors_per_cluster * self.bytes_per_sector
    }

    /// Returns the first sector of the data region (cluster 2).
    #[must_use]
    pub const fn data_start_sector(&self) -> u64 {
        self.reserved_sectors as u64 + self.fat_count as u64 * self.fat_sectors as u64
    }

    /// Returns the usable data capacity in bytes.
    #[must_use]
    pub const fn data_bytes(&self) -> u64 {
        self.cluster_count as u64 * self.cluster_size() as u64
    }

    /// Converts a sector number to a byte offset.
    pub(super) const fn offset(&self, sector: u64) -> u64 {
        sector * self.bytes_per_sector as u64
    }

    /// Returns the sector size as a buffer length.
    pub(super) const fn sector_len(&self) -> usize {
        // Sector sizes are at most 4096, so this never truncates.
        self.bytes_per_sector as usize
    }
}

/// Returns the default cluster size in bytes for a volume of `size_bytes`.
///
/// The table follows the FAT32 defaults of the Microsoft specification and
/// extends them beyond 32 GiB with 32 KiB clusters, and 64 KiB clusters above
/// 2 TiB.
#[must_use]
pub const fn auto_cluster_size(size_bytes: u64) -> u32 {
    const MIB: u64 = 1024 * 1024;
    const GIB: u64 = 1024 * MIB;
    if size_bytes <= 260 * MIB {
        512
    } else if size_bytes < 8 * GIB {
        4096
    } else if size_bytes < 16 * GIB {
        8192
    } else if size_bytes < 32 * GIB {
        16_384
    } else if size_bytes < 2048 * GIB {
        32_768
    } else {
        65_536
    }
}
