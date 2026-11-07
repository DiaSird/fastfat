//! Human readable byte counts.

use std::fmt::Write as _;

/// Binary unit suffixes, in increasing order.
const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];

/// Formats a byte count with a binary unit suffix, e.g. `931.5 GiB`.
#[must_use]
pub fn format_size(bytes: u64) -> String {
    let mut text = String::new();
    write_size(&mut text, bytes);
    text
}

/// Appends a byte count with a binary unit suffix to `out`.
#[allow(clippy::cast_precision_loss)]
pub fn write_size(out: &mut String, bytes: u64) {
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    let _ = if unit == 0 {
        write!(out, "{bytes} B")
    } else {
        write!(out, "{value:.1} {}", UNITS[unit])
    };
}

#[cfg(test)]
mod tests {
    use super::format_size;

    #[test]
    fn sizes() {
        assert_eq!(format_size(512), "512 B");
        assert_eq!(format_size(32_768), "32.0 KiB");
        assert_eq!(format_size(1_000_204_886_016), "931.5 GiB");
    }
}
