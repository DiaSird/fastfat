//! Japanese text needs a font that egui does not ship: one of the Windows
//! system fonts is added as a fallback.

use std::path::Path;

use eframe::egui;
use egui::epaint::text::{FontInsert, FontPriority, InsertFontFamily};

/// Fonts searched in `%WINDIR%\Fonts` to render Japanese text.
const JAPANESE_FONTS: [&str; 3] = ["meiryo.ttc", "YuGothM.ttc", "msgothic.ttc"];

/// Adds a Windows system font as fallback so that Japanese text renders.
pub(super) fn install_japanese_font(ctx: &egui::Context) {
    let windir = std::env::var_os("WINDIR");
    let fonts = windir
        .as_deref()
        .map_or_else(|| Path::new(r"C:\Windows"), Path::new)
        .join("Fonts");
    let Some(bytes) = JAPANESE_FONTS
        .iter()
        .find_map(|name| std::fs::read(fonts.join(name)).ok())
    else {
        return;
    };
    let families = [egui::FontFamily::Proportional, egui::FontFamily::Monospace]
        .into_iter()
        .map(|family| InsertFontFamily {
            family,
            priority: FontPriority::Lowest,
        })
        .collect();
    ctx.add_font(FontInsert::new(
        "japanese",
        egui::FontData::from_owned(bytes),
        families,
    ));
}
