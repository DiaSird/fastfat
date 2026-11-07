//! The FAT32 formatting page.

use eframe::egui;
use fastfat_device::{DriveKind, PartitionStyle};
use fastfat_fat32::{CLUSTER_SIZES, Geometry, Layout, VolumeLabel};

use super::i18n::{t, tf};
use super::{Confirm, FastFatApp, Status, find_disk, find_drive};

/// Display names of [`CLUSTER_SIZES`], in the same order.
const CLUSTER_LABELS: [&str; CLUSTER_SIZES.len()] = [
    "512 B", "1 KiB", "2 KiB", "4 KiB", "8 KiB", "16 KiB", "32 KiB", "64 KiB",
];

/// Largest cluster size recommended for the Wii U.
const WII_U_MAX_CLUSTER: u32 = 32_768;

/// Returns the display name of a cluster size from [`CLUSTER_SIZES`].
pub(super) fn cluster_label(size: u32) -> &'static str {
    CLUSTER_SIZES
        .iter()
        .position(|&s| s == size)
        .map_or("?", |i| CLUSTER_LABELS[i])
}

impl FastFatApp {
    /// Draws the drive selector.
    pub(super) fn drive_section(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(t("format.drive"));
            // Borrow `drives` and `selected` separately so that the combo
            // box can show the description without copying it.
            let text = find_drive(&self.drives, self.selected)
                .map_or_else(|| t("format.no_drives"), |d| d.description.as_str());
            egui::ComboBox::from_id_salt("drive")
                .selected_text(text)
                .width(420.0)
                .show_ui(ui, |ui| {
                    for drive in &self.drives {
                        ui.selectable_value(
                            &mut self.selected,
                            Some(drive.letter),
                            drive.description.as_str(),
                        );
                    }
                });
            if ui.button(t("common.refresh")).clicked() {
                self.refresh();
            }
        });
        if ui
            .checkbox(&mut self.show_fixed, t("format.show_fixed"))
            .changed()
        {
            self.refresh();
        }
        let Some(drive) = self.selected_drive() else {
            return;
        };
        let warn = ui.visuals().warn_fg_color;
        if drive.kind == DriveKind::Fixed {
            ui.colored_label(warn, t("format.fixed_warning"));
        }
        if drive.partition_style == Some(PartitionStyle::Gpt) && !self.wii_u {
            ui.colored_label(warn, t("format.gpt_warning"));
        }
    }

    /// Draws the format options and the start button.
    pub(super) fn options_section(&mut self, ui: &mut egui::Ui) {
        // Cluster size the automatic mode would pick (assuming 512 byte sectors).
        let auto = self.selected_drive().and_then(|d| d.size).map(|size| {
            Layout::compute(&Geometry::for_image(size), None).map_or_else(
                |_| t("format.too_small"),
                |layout| cluster_label(layout.cluster_size()),
            )
        });

        egui::Grid::new("options")
            .num_columns(2)
            .spacing([12.0, 8.0])
            .show(ui, |ui| {
                ui.label(t("format.cluster_size"));
                ui.horizontal(|ui| {
                    let text = self
                        .cluster_size
                        .map_or_else(|| t("format.auto"), cluster_label);
                    egui::ComboBox::from_id_salt("cluster")
                        .selected_text(text)
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut self.cluster_size, None, t("format.auto"));
                            for (size, label) in CLUSTER_SIZES.into_iter().zip(CLUSTER_LABELS) {
                                ui.selectable_value(&mut self.cluster_size, Some(size), label);
                            }
                        });
                    if let (None, Some(auto)) = (self.cluster_size, auto) {
                        ui.weak(auto);
                    }
                });
                ui.end_row();

                ui.label(t("format.label"));
                ui.add(
                    egui::TextEdit::singleline(&mut self.label)
                        .char_limit(11)
                        .hint_text(t("format.label_hint")),
                );
                ui.end_row();

                ui.label("");
                ui.checkbox(&mut self.quick, t("format.quick"));
                ui.end_row();

                ui.label("");
                ui.vertical(|ui| {
                    ui.checkbox(&mut self.wii_u, t("format.wiiu"));
                    if self.wii_u {
                        ui.weak(t("format.wiiu_note"));
                        if self
                            .cluster_size
                            .is_some_and(|size| size > WII_U_MAX_CLUSTER)
                        {
                            ui.colored_label(ui.visuals().warn_fg_color, t("format.wiiu_cluster"));
                        }
                    }
                });
                ui.end_row();
            });

        ui.separator();
        let button = egui::Button::new(t("format.start")).min_size(egui::vec2(140.0, 32.0));
        if ui.add_enabled(self.selected.is_some(), button).clicked() {
            match VolumeLabel::parse(&self.label) {
                Ok(label) => self.confirm = Some(Confirm::Format(label)),
                Err(err) => self.status = Status::Error(err.to_string()),
            }
        }
    }

    /// Draws the body of the format confirmation.
    pub(super) fn format_confirmation(&self, ui: &mut egui::Ui) {
        ui.heading(t("format.confirm_title"));
        ui.add_space(4.0);
        let error = ui.visuals().error_fg_color;
        if let Some(drive) = self.selected_drive() {
            ui.label(drive.description.as_str());
            if let (true, Some(disk)) = (self.wii_u, drive.disk) {
                ui.colored_label(error, tf("format.confirm_mbr", &[("disk", &disk)]));
                if let Some(info) = find_disk(&self.disks, Some(disk)) {
                    ui.label(info.description.as_str());
                }
            }
        }
        ui.colored_label(error, t("format.confirm_erase"));
    }
}
