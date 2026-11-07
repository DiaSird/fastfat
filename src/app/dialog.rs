//! The confirmation dialog shown before destructive operations.

use eframe::egui;

use super::i18n::t;
use super::{Confirm, FastFatApp};

impl FastFatApp {
    /// Draws the confirmation dialog when it is open and starts the
    /// confirmed job.
    pub(super) fn confirm_dialog(&mut self, ctx: &egui::Context) {
        let Some(confirm) = self.confirm else {
            return;
        };
        let mut start = false;
        let mut close = false;
        let modal = egui::Modal::new(egui::Id::unique("confirm")).show(ctx, |ui| {
            match confirm {
                Confirm::Format(_) => self.format_confirmation(ui),
                Confirm::Clone => self.clone_confirmation(ui),
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                start = ui.button(t("dialog.run")).clicked();
                close = ui.button(t("dialog.cancel")).clicked();
            });
        });
        if start {
            self.confirm = None;
            match confirm {
                Confirm::Format(label) => self.start_format(label),
                Confirm::Clone => self.start_clone(),
            }
        } else if close || modal.should_close() {
            self.confirm = None;
        }
    }
}
