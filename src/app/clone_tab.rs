//! The disk cloning page.

use eframe::egui;
use fastfat_device::{CloneMode, DiskInfo};
use fastfat_units::format_size;

use super::i18n::{t, tf};
use super::{Confirm, FastFatApp, find_disk};

/// Default size of the empty Wii U storage partition in GiB.
pub(super) const DEFAULT_STORAGE_GIB: u32 = 64;

/// Bytes per GiB.
const GIB: u64 = 1024 * 1024 * 1024;

/// Layout of the clone destination, as chosen in the GUI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CloneLayout {
    /// Copy every sector.
    Verbatim,
    /// Grow FAT32 and move the other partitions verbatim to the end.
    Expand,
    /// Grow FAT32 and create an empty Wii U storage partition.
    WiiU,
}

impl FastFatApp {
    /// Returns the clone mode for the current GUI settings.
    pub(super) fn clone_mode(&self) -> CloneMode {
        match self.clone_layout {
            CloneLayout::Verbatim => CloneMode::Verbatim,
            CloneLayout::Expand => CloneMode::ExpandFat32,
            CloneLayout::WiiU => CloneMode::WiiUFreshStorage {
                storage_bytes: u64::from(self.storage_gib) * GIB,
            },
        }
    }

    /// Returns why the selected disks cannot be cloned, if they cannot.
    fn clone_problem(&self) -> Option<&'static str> {
        let source = find_disk(&self.disks, self.clone_source)?;
        let Some(destination) = find_disk(&self.disks, self.clone_destination) else {
            return Some(t("clone.pick_destination"));
        };
        if source.number == destination.number {
            Some(t("clone.same_disk"))
        } else if destination.is_system {
            Some(t("clone.system_disk"))
        } else if destination.bytes_per_sector != source.bytes_per_sector {
            Some(t("clone.sector_mismatch"))
        } else if self.clone_layout != CloneLayout::WiiU && destination.size < source.size {
            Some(t("clone.too_small"))
        } else if self.clone_layout == CloneLayout::WiiU
            && u64::from(self.storage_gib) * GIB >= destination.size
        {
            Some(t("clone.storage_too_big"))
        } else {
            None
        }
    }

    /// Draws the clone page.
    pub(super) fn clone_section(&mut self, ui: &mut egui::Ui) {
        ui.label(t("clone.intro"));
        ui.add_space(4.0);
        egui::Grid::new("clone")
            .num_columns(2)
            .spacing([12.0, 8.0])
            .show(ui, |ui| {
                ui.label(t("clone.source"));
                disk_combo(
                    ui,
                    "clone_source",
                    &self.disks,
                    &mut self.clone_source,
                    false,
                );
                ui.end_row();

                ui.label(t("clone.destination"));
                disk_combo(
                    ui,
                    "clone_destination",
                    &self.disks,
                    &mut self.clone_destination,
                    true,
                );
                ui.end_row();

                ui.label(t("clone.layout"));
                ui.vertical(|ui| self.layout_choice(ui));
                ui.end_row();

                ui.label("");
                ui.checkbox(&mut self.new_signature, t("clone.new_signature"));
                ui.end_row();
            });
        if ui.button(t("common.refresh")).clicked() {
            self.refresh();
        }

        let problem = self.clone_problem();
        if let Some(problem) = problem {
            ui.colored_label(ui.visuals().warn_fg_color, problem);
        }

        ui.separator();
        let button = egui::Button::new(t("clone.start")).min_size(egui::vec2(140.0, 32.0));
        if ui.add_enabled(problem.is_none(), button).clicked() {
            self.confirm = Some(Confirm::Clone);
        }
    }

    /// Draws the layout radio buttons and their options.
    fn layout_choice(&mut self, ui: &mut egui::Ui) {
        ui.radio_value(
            &mut self.clone_layout,
            CloneLayout::WiiU,
            t("clone.layout_wiiu"),
        );
        if self.clone_layout == CloneLayout::WiiU {
            ui.horizontal(|ui| {
                ui.label(t("clone.storage_area"));
                ui.add(
                    egui::DragValue::new(&mut self.storage_gib)
                        .range(0..=2048)
                        .suffix(" GiB"),
                );
                if let Some(destination) = find_disk(&self.disks, self.clone_destination) {
                    let storage = u64::from(self.storage_gib) * GIB;
                    ui.weak(format!(
                        "FAT32 ≒ {}",
                        format_size(destination.size.saturating_sub(storage))
                    ));
                }
            });
            ui.weak(t("clone.wiiu_note1"));
            ui.weak(t("clone.wiiu_note2"));
        }
        ui.radio_value(
            &mut self.clone_layout,
            CloneLayout::Expand,
            t("clone.layout_expand"),
        );
        ui.radio_value(
            &mut self.clone_layout,
            CloneLayout::Verbatim,
            t("clone.layout_verbatim"),
        );
    }

    /// Draws the body of the clone confirmation.
    pub(super) fn clone_confirmation(&self, ui: &mut egui::Ui) {
        ui.heading(t("clone.confirm_title"));
        ui.add_space(4.0);
        if let Some(source) = find_disk(&self.disks, self.clone_source) {
            ui.label(t("clone.source"));
            ui.label(source.description.as_str());
        }
        if let Some(destination) = find_disk(&self.disks, self.clone_destination) {
            ui.label(t("clone.destination"));
            ui.label(destination.description.as_str());
            match self.clone_layout {
                CloneLayout::WiiU => {
                    ui.label(tf("clone.confirm_wiiu", &[("gib", &self.storage_gib)]))
                }
                CloneLayout::Expand => ui.label(t("clone.confirm_expand")),
                CloneLayout::Verbatim => ui.label(t("clone.confirm_verbatim")),
            };
            ui.colored_label(
                ui.visuals().error_fg_color,
                tf("clone.confirm_overwrite", &[("disk", &destination.number)]),
            );
        }
    }
}

/// Draws a combo box selecting a disk by number.
///
/// With `writable`, the system disk is shown but cannot be selected.
fn disk_combo(
    ui: &mut egui::Ui,
    id: &str,
    disks: &[DiskInfo],
    selected: &mut Option<u32>,
    writable: bool,
) {
    let text =
        find_disk(disks, *selected).map_or_else(|| t("common.choose"), |d| d.description.as_str());
    egui::ComboBox::from_id_salt(id)
        .selected_text(text)
        .width(460.0)
        .show_ui(ui, |ui| {
            for disk in disks {
                ui.add_enabled_ui(!(writable && disk.is_system), |ui| {
                    ui.selectable_value(selected, Some(disk.number), disk.description.as_str());
                });
            }
        });
}
