//! The PC pane: a minimal Explorer showing one folder of the PC. Wii U rows
//! dropped on it are copied into that folder.

use std::path::{Path, PathBuf};

use eframe::egui;
use fastfat_units::format_size;

use super::rows::{Focus, PARENT_ROW, icon, list_height};
use super::{Dragged, RowKind, clean_path};
use crate::app::i18n::t;

/// An entry of a PC folder.
struct LocalEntry {
    /// File name.
    name: String,
    /// Whether it is a directory.
    is_dir: bool,
    /// Size in bytes.
    size: u64,
}

/// The PC pane: a minimal Explorer that is the drop target of copies.
pub(in crate::app) struct LocalPane {
    /// Folder shown; `None` shows the drives.
    dir: Option<PathBuf>,
    /// Text of the path bar.
    path_text: String,
    /// Entries of the folder shown.
    entries: Vec<LocalEntry>,
    /// Why the folder could not be listed.
    error: Option<String>,
}

impl Default for LocalPane {
    fn default() -> Self {
        let start = std::env::var_os("USERPROFILE")
            .map(|home| PathBuf::from(home).join("Downloads"))
            .filter(|p| p.is_dir());
        let mut pane = Self {
            dir: None,
            path_text: String::new(),
            entries: Vec::new(),
            error: None,
        };
        pane.open(start);
        pane
    }
}

impl LocalPane {
    /// Returns the folder shown, if any.
    pub(in crate::app) fn dir(&self) -> Option<&Path> {
        self.dir.as_deref()
    }

    /// Shows `dir` (`None` for the drives).
    pub(in crate::app) fn open(&mut self, dir: Option<PathBuf>) {
        self.path_text = dir
            .as_ref()
            .map(|d| d.display().to_string())
            .unwrap_or_default();
        self.dir = dir;
        self.reload();
    }

    /// Lists the folder shown again.
    pub(in crate::app) fn reload(&mut self) {
        self.error = None;
        self.entries = match &self.dir {
            None => ('C'..='Z')
                .map(|letter| format!("{letter}:\\"))
                .filter(|root| Path::new(root).is_dir())
                .map(|root| LocalEntry {
                    name: root,
                    is_dir: true,
                    size: 0,
                })
                .collect(),
            Some(dir) => match std::fs::read_dir(dir) {
                Ok(read) => {
                    let mut entries: Vec<LocalEntry> = read
                        .filter_map(Result::ok)
                        .filter_map(|e| {
                            let meta = e.metadata().ok()?;
                            Some(LocalEntry {
                                name: e.file_name().to_string_lossy().into_owned(),
                                is_dir: meta.is_dir(),
                                size: meta.len(),
                            })
                        })
                        .collect();
                    entries.sort_by(|a, b| {
                        (!a.is_dir, a.name.to_lowercase()).cmp(&(!b.is_dir, b.name.to_lowercase()))
                    });
                    entries
                }
                Err(err) => {
                    self.error = Some(err.to_string());
                    Vec::new()
                }
            },
        };
    }

    /// Draws the pane and returns a Wii U row dropped on it.
    pub(in crate::app) fn draw(&mut self, ui: &mut egui::Ui) -> Option<Dragged> {
        let mut open = None;
        // `Some(None)` is the drive list: the parent of a drive root.
        let parent = self.dir.as_ref().map(|d| d.parent().map(Path::to_path_buf));
        ui.horizontal(|ui| {
            ui.strong("PC");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button(t("pane.reload")).clicked() {
                    self.reload();
                }
                if ui.small_button(t("pane.drives")).clicked() {
                    open = Some(None);
                }
            });
        });
        let edit =
            ui.add(egui::TextEdit::singleline(&mut self.path_text).desired_width(f32::INFINITY));
        if edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            let typed = PathBuf::from(clean_path(&self.path_text));
            open = Some(Some(typed));
        }
        // Keep the lists of both panes at the same height.
        ui.add_space(ui.spacing().interact_size.y + ui.spacing().item_spacing.y);

        let folder = self.path_text.clone();
        let mut focus = Focus::load(ui, "local", &folder);
        let frame = egui::Frame::group(ui.style()).inner_margin(4.0);
        let (_, dropped) = ui.dnd_drop_zone::<Dragged, ()>(frame, |ui| {
            egui::ScrollArea::vertical()
                .id_salt("local")
                .max_height(list_height(ui))
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    if let Some(error) = &self.error {
                        ui.colored_label(ui.visuals().error_fg_color, error);
                    }
                    if let Some(parent) = &parent {
                        if focus.row(ui, PARENT_ROW, "📁 ..").1 {
                            open = Some(parent.clone());
                        }
                    }
                    for (index, entry) in self.entries.iter().enumerate() {
                        ui.horizontal(|ui| {
                            let kind = if entry.is_dir {
                                RowKind::Directory
                            } else {
                                RowKind::File
                            };
                            let text = format!("{} {}", icon(kind), entry.name);
                            let row_id = isize::try_from(index).unwrap_or(isize::MAX);
                            let (_, activated) = focus.row(ui, row_id, &text);
                            if entry.is_dir && activated {
                                open = Some(Some(match &self.dir {
                                    Some(dir) => dir.join(&entry.name),
                                    None => PathBuf::from(&entry.name),
                                }));
                            }
                            if !entry.is_dir {
                                ui.weak(format_size(entry.size));
                            }
                        });
                    }
                });
        });
        focus.store(ui);
        ui.weak(match &self.dir {
            Some(_) => t("pane.local_hint"),
            None => t("pane.open_destination"),
        });

        if let Some(dir) = open {
            self.open(dir);
        }
        dropped.map(|d| *d)
    }
}
