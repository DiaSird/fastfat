//! Two-pane file browsing in the style of WinSCP: the PC on the left, the
//! Wii U (read-only) on the right. Rows of the Wii U pane are dragged onto
//! the PC pane to copy them into the PC folder shown there.
//!
//! In both panes a click selects a row; a double click, or a click on the
//! selected row, opens a folder. The `..` row at the top opens the parent.
//!
//! * `remote.rs`: the Wii U pane ([`draw_remote`]).
//! * `local.rs`: the PC pane ([`LocalPane`]), the drop target.
//! * `rows.rs`: pieces shared by both panes (row selection, drag preview).
//! * this file: the data passed between the panes and the pages.

mod local;
mod remote;
mod rows;

pub(super) use local::LocalPane;
pub(super) use remote::draw_remote;

/// Kind of a listed entry.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum RowKind {
    /// A directory that can be entered.
    Directory,
    /// A file.
    File,
    /// A link (listed, not copied).
    Link,
}

/// One listed entry of the Wii U pane.
pub(super) struct Row<'a> {
    /// Display name.
    pub(super) name: &'a str,
    /// Kind.
    pub(super) kind: RowKind,
    /// File size in bytes.
    pub(super) size: u64,
}

/// What the user asked for in the Wii U pane.
pub(super) enum Action {
    /// Go to the parent directory.
    Up,
    /// Enter the directory at this index.
    Enter(usize),
    /// Go to this path (a shortcut).
    Goto(&'static str),
    /// Copy the entries at these indices into the PC folder.
    Copy(Vec<usize>),
}

/// Payload of a dragged Wii U row.
#[derive(Clone, Copy)]
pub(super) struct Dragged {
    /// Pane the row belongs to.
    source: &'static str,
    /// Index of the row.
    index: usize,
}

/// Returns the rows to copy for a row dropped on the PC pane: every checked
/// row when the dropped row is checked, else just that row. Rows of another
/// pane are ignored.
pub(super) fn dropped_rows(
    dragged: Dragged,
    source: &'static str,
    selected: &[bool],
) -> Option<Vec<usize>> {
    if dragged.source != source {
        return None;
    }
    Some(if selected.get(dragged.index).copied().unwrap_or(false) {
        checked(selected)
    } else {
        vec![dragged.index]
    })
}

/// Returns the indices of the checked rows.
fn checked(selected: &[bool]) -> Vec<usize> {
    selected
        .iter()
        .enumerate()
        .filter_map(|(i, &s)| s.then_some(i))
        .collect()
}

/// Removes blanks and the quotes that Explorer's "Copy as path" adds.
pub(super) fn clean_path(text: &str) -> &str {
    text.trim().trim_matches('"').trim()
}
