//! The progress panel above the progress bar: the question about existing
//! files, the files being received, their folder, the totals, speed and
//! time left, and the cancel button.

use std::sync::atomic::Ordering;
use std::time::Duration;

use eframe::egui;
use fastfat_ftp::{self as ftp, ExistingFiles};
use fastfat_units::format_size;

use super::Transfers;
use super::progress::{ActiveFile, TransferView};
use super::speed::format_duration;
use crate::app::i18n::{t, tf};

impl Transfers {
    /// Draws the transfer above the progress bar.
    pub(in crate::app) fn draw(&self, ui: &mut egui::Ui) {
        let Some(running) = &self.running else {
            return;
        };
        let Ok(mut view) = running.view.lock() else {
            return;
        };
        if view.planned && view.question.is_none() {
            let done = view.total.bytes_done + view.in_flight(None);
            view.speed.sample(done);
        }
        let cancelling = running.cancel.load(Ordering::Relaxed);
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            if let Some((existing, complete)) = view.question {
                ui.colored_label(
                    ui.visuals().warn_fg_color,
                    tf(
                        "transfer.existing",
                        &[("existing", &existing), ("complete", &complete)],
                    ),
                );
                ui.horizontal(|ui| {
                    let only_new = ui.button(t("transfer.only_new")).clicked();
                    let overwrite = ui.button(t("transfer.overwrite")).clicked();
                    let cancel = ui.button(t("job.cancel")).clicked();
                    let answer = if only_new {
                        Some(Some(ExistingFiles::SkipComplete))
                    } else if overwrite {
                        Some(Some(ExistingFiles::Overwrite))
                    } else if cancel {
                        Some(None)
                    } else {
                        None
                    };
                    if let Some(answer) = answer {
                        let _ = running.answer.send(answer);
                    }
                });
                return;
            }
            if view.planned {
                for active in view.active.iter().flatten() {
                    draw_file(ui, &view, active);
                }
            } else {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(t("transfer.planning"));
                });
            }
            let total = view.total;
            let done = total.bytes_done + view.in_flight(None);
            ui.label(tf(
                "transfer.total",
                &[
                    ("done", &total.files_done),
                    ("files", &total.files),
                    ("bytes_done", &format_size(done)),
                    ("bytes", &format_size(total.bytes)),
                ],
            ));
            if view.planned {
                let remaining = total.bytes.saturating_sub(done);
                let speed = view.speed.bytes_per_second();
                #[allow(
                    clippy::cast_possible_truncation,
                    clippy::cast_sign_loss,
                    clippy::cast_precision_loss
                )]
                let eta = speed.filter(|&bps| bps > 0.0).map_or_else(
                    || t("transfer.eta_unknown").to_owned(),
                    |bps| format_duration(Duration::from_secs_f64(remaining as f64 / bps)),
                );
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let speed = speed.map_or_else(|| "-".to_owned(), |bps| format_size(bps as u64));
                ui.label(tf(
                    "transfer.speed",
                    &[
                        ("speed", &speed),
                        ("elapsed", &format_duration(view.speed.elapsed())),
                        ("eta", &eta),
                    ],
                ));
            }
            if !self.queue.is_empty() {
                ui.weak(tf("transfer.queued", &[("count", &self.queue.len())]));
            }
            #[allow(clippy::cast_precision_loss)]
            let fraction = if total.bytes == 0 {
                0.0
            } else {
                (done as f64 / total.bytes as f64) as f32
            };
            ui.horizontal(|ui| {
                ui.add(
                    egui::ProgressBar::new(fraction)
                        .show_percentage()
                        .desired_width(ui.available_width() - 110.0),
                );
                if ui
                    .add_enabled(!cancelling, egui::Button::new(t("job.cancel")))
                    .clicked()
                {
                    running.cancel.store(true, Ordering::Relaxed);
                }
            });
        });
    }
}

/// Draws a file being downloaded and the progress of its folder.
fn draw_file(ui: &mut egui::Ui, view: &TransferView, active: &ActiveFile) {
    let folder = ftp::parent(&active.remote);
    let name = active.remote.rsplit('/').next().unwrap_or(&active.remote);
    ui.horizontal(|ui| {
        ui.label(tf("transfer.file", &[("name", &name)]));
        ui.weak(tf(
            "transfer.file_progress",
            &[
                ("received", &format_size(active.received)),
                ("size", &format_size(active.size)),
            ],
        ));
    });
    if let Some(progress) = view.folders.get(&folder) {
        ui.add(
            egui::Label::new(egui::RichText::new(tf(
                "transfer.folder",
                &[
                    ("folder", &folder),
                    ("done", &progress.files_done),
                    ("files", &progress.files),
                    (
                        "bytes_done",
                        &format_size(progress.bytes_done + view.in_flight(Some(&folder))),
                    ),
                    ("bytes", &format_size(progress.bytes)),
                ],
            )))
            .truncate(),
        );
    }
}
