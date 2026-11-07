//! Pieces shared by both panes: row icons, list height, the selected row
//! and the preview shown next to the pointer while dragging.

use eframe::egui;

use super::RowKind;

/// Height kept free below a list for the hint line under it.
pub(super) const RESERVED_HEIGHT: f32 = 28.0;

/// Focus value of the `..` row.
pub(super) const PARENT_ROW: isize = -1;

/// Returns the icon of an entry kind.
pub(super) const fn icon(kind: RowKind) -> &'static str {
    match kind {
        RowKind::Directory => "📁",
        RowKind::File => "📄",
        RowKind::Link => "🔗",
    }
}

/// Returns the height available to a pane's list.
pub(super) fn list_height(ui: &egui::Ui) -> f32 {
    (ui.available_height() - RESERVED_HEIGHT).max(80.0)
}

/// The selected row of a pane, kept in egui memory and forgotten when the
/// folder changes.
pub(super) struct Focus {
    /// Memory key.
    pub(super) id: egui::Id,
    /// Folder the focus belongs to.
    pub(super) folder: String,
    /// Selected row (`PARENT_ROW` for `..`).
    pub(super) row: Option<isize>,
}

impl Focus {
    /// Loads the focus of the pane `key` in `folder`.
    pub(super) fn load(ui: &egui::Ui, key: &str, folder: &str) -> Self {
        let id = ui.scope_id().with(("focus", key));
        let stored: Option<(String, isize)> = ui.ctx().data_mut(|d| d.get_temp(id));
        let row = stored.and_then(|(f, row)| (f == folder).then_some(row));
        Self {
            id,
            folder: folder.to_owned(),
            row,
        }
    }

    /// Stores the focus.
    pub(super) fn store(&self, ui: &egui::Ui) {
        let value = self.row.map(|row| (self.folder.clone(), row));
        ui.ctx().data_mut(|d| {
            if let Some(value) = value {
                d.insert_temp(self.id, value);
            } else {
                d.remove::<(String, isize)>(self.id);
            }
        });
    }

    /// Draws a selectable row; returns its response and whether it was
    /// activated (double click, or a click on the selected row).
    pub(super) fn row(
        &mut self,
        ui: &mut egui::Ui,
        row: isize,
        text: &str,
    ) -> (egui::Response, bool) {
        let selected = self.row == Some(row);
        let response = ui.add(egui::Button::selectable(selected, text).frame_when_inactive(false));
        let activated = response.double_clicked() || (response.clicked() && selected);
        if response.clicked() {
            self.row = Some(row);
        }
        (response, activated)
    }
}

/// Shows `text` next to the pointer while `response` is being dragged.
pub(super) fn drag_preview(ui: &egui::Ui, response: &egui::Response, text: &str) {
    if !response.dragged() {
        return;
    }
    let Some(pos) = ui.ctx().pointer_latest_pos() else {
        return;
    };
    egui::Area::new(ui.scope_id().with("drag_preview"))
        .fixed_pos(pos + egui::vec2(14.0, 10.0))
        .order(egui::Order::Tooltip)
        .interactable(false)
        .show(ui.ctx(), |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| ui.label(text));
        });
}
