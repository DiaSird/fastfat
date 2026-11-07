//! The Wii U pane: shortcuts, path, selection tools and the rows that can be
//! dragged to the PC pane.

use eframe::egui;
use fastfat_units::format_size;

use super::rows::{Focus, PARENT_ROW, drag_preview, icon, list_height};
use super::{Action, Dragged, Row, RowKind, checked};
use crate::app::i18n::{t, tf};

/// Draws the Wii U pane: title with the shortcuts at the top right, path
/// bar, selection tools and the rows.
///
/// `source` tells panes apart so that a row of one page is never copied by
/// another. `selected` holds the check marks; dragging a checked row copies
/// every checked row.
pub(in crate::app) fn draw_remote(
    ui: &mut egui::Ui,
    source: &'static str,
    path: &str,
    can_up: bool,
    rows: &[Row<'_>],
    selected: &mut Vec<bool>,
    shortcuts: &[(&'static str, &'static str)],
) -> Option<Action> {
    let mut action = None;
    selected.resize(rows.len(), false);
    let count = selected.iter().filter(|&&s| s).count();

    ui.horizontal(|ui| {
        ui.strong(t("pane.remote_title"));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            for &(label, target) in shortcuts.iter().rev() {
                if ui.small_button(t(label)).on_hover_text(target).clicked() {
                    action = Some(Action::Goto(target));
                }
            }
        });
    });
    ui.add(egui::Label::new(egui::RichText::new(path).monospace()).truncate());
    ui.horizontal(|ui| {
        if ui.small_button(t("pane.select_all")).clicked() {
            selected.fill(true);
        }
        if ui.small_button(t("pane.select_none")).clicked() {
            selected.fill(false);
        }
        if count > 0
            && ui
                .small_button(tf("pane.copy_selected", &[("count", &count)]))
                .clicked()
        {
            action = Some(Action::Copy(checked(selected)));
        }
    });

    let mut focus = Focus::load(ui, source, path);
    let frame = egui::Frame::group(ui.style()).inner_margin(4.0);
    frame.show(ui, |ui| {
        egui::ScrollArea::vertical()
            .id_salt((source, "remote"))
            .max_height(list_height(ui))
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if can_up {
                    ui.horizontal(|ui| {
                        ui.add_space(ui.spacing().interact_size.y + ui.spacing().item_spacing.x);
                        if focus.row(ui, PARENT_ROW, "📁 ..").1 {
                            action = Some(Action::Up);
                        }
                    });
                } else if rows.is_empty() {
                    ui.weak(t("pane.empty"));
                }
                for (index, row) in rows.iter().enumerate() {
                    ui.horizontal(|ui| {
                        ui.checkbox(&mut selected[index], "");

                        let text = format!("{} {}", icon(row.kind), row.name);
                        let row_id = isize::try_from(index).unwrap_or(isize::MAX);
                        let (response, activated) = focus.row(ui, row_id, &text);
                        let response = response.interact(egui::Sense::drag());

                        response.dnd_set_drag_payload(Dragged { source, index });

                        let preview = if selected[index] && count > 1 {
                            tf("pane.drag_count", &[("count", &count)])
                        } else {
                            text
                        };

                        drag_preview(ui, &response, &preview);

                        if row.kind == RowKind::Directory && activated {
                            action = Some(Action::Enter(index));
                        }

                        if row.kind == RowKind::File {
                            ui.weak(format_size(row.size));
                        }
                    });
                }
            });
    });
    focus.store(ui);
    ui.weak(t("pane.remote_hint"));
    action
}
