//! FAT volume label validation and encoding.

use super::FormatError;

/// Characters that are not allowed in a FAT volume label.
const INVALID_LABEL_CHARS: &str = "\"*+,./:;<=>?[\\]|";

/// Offset of the volume label inside a FAT32 boot sector.
const BOOT_SECTOR_LABEL: usize = 71;

/// A validated FAT volume label in its on-disk form: 11 upper-case ASCII
/// bytes padded with spaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct VolumeLabel([u8; 11]);

impl VolumeLabel {
    /// Label used when the user does not provide one.
    pub const NO_NAME: Self = Self(*b"NO NAME    ");

    /// Validates `label` and converts it into the on-disk form.
    ///
    /// Lower-case letters are converted to upper case. An empty or all-blank
    /// label yields [`Self::NO_NAME`].
    ///
    /// # Errors
    ///
    /// Returns [`FormatError::InvalidLabel`] when the label is longer than 11
    /// characters, is not ASCII or starts with a space, and
    /// [`FormatError::InvalidLabelChar`] when it contains a control or
    /// reserved character.
    pub fn parse(label: &str) -> Result<Self, FormatError> {
        let label = label.trim_end();
        if label.is_empty() {
            return Ok(Self::NO_NAME);
        }
        if !label.is_ascii() {
            return Err(FormatError::InvalidLabel(
                "only ASCII characters are allowed",
            ));
        }
        if label.len() > 11 {
            return Err(FormatError::InvalidLabel(
                "at most 11 characters are allowed",
            ));
        }
        if label.starts_with(' ') {
            return Err(FormatError::InvalidLabel("must not start with a space"));
        }
        if let Some(c) = label
            .chars()
            .find(|c| c.is_ascii_control() || INVALID_LABEL_CHARS.contains(*c))
        {
            return Err(FormatError::InvalidLabelChar(c));
        }

        let mut encoded = [b' '; 11];
        for (dst, src) in encoded.iter_mut().zip(label.bytes()) {
            *dst = src.to_ascii_uppercase();
        }
        Ok(Self(encoded))
    }

    /// Reads the label stored in an existing FAT32 boot sector.
    ///
    /// Labels that would not pass [`Self::parse`] yield [`Self::NO_NAME`].
    #[must_use]
    pub fn from_boot_sector(boot: &[u8]) -> Self {
        boot.get(BOOT_SECTOR_LABEL..BOOT_SECTOR_LABEL + 11)
            .and_then(|bytes| std::str::from_utf8(bytes).ok())
            .and_then(|text| Self::parse(text).ok())
            .unwrap_or_default()
    }

    /// Returns the 11 on-disk bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 11] {
        &self.0
    }
}

impl Default for VolumeLabel {
    fn default() -> Self {
        Self::NO_NAME
    }
}
