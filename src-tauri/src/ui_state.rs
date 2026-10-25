//! UI state: what the window remembers between runs, as opposed to settings. Panel sizes, column
//! widths, open tabs, the Console, Files panel and diff header choices, dialog sizes, and for each
//! repository the refs hidden from the graph.
//!
//! Stored in the SQLite database as one row per value (`storage::ui_prefs`), written constantly
//! and never hand-edited. Kept out of `settings.json` so the churn never races a hand edit there.
//! Before 2026-09-30 it was `ui-state.json`, which is imported once and renamed.

use serde::{Deserialize, Deserializer, Serialize};
use specta::Type;

use crate::settings::SortedMap;

#[derive(Debug, Clone, Default, Serialize, Deserialize, Type)]
#[serde(default, rename_all = "camelCase")]
pub struct UiState {
    pub panels: Panels,
    pub graph_columns: GraphColumns,
    pub session: Session,
    pub console: ConsoleView,
    pub files: FilesView,
    pub diff: DiffView,
    /// Dialogs the user has dragged bigger, by the name each dialog gives `DialogContent`.
    pub dialog_sizes: SortedMap<DialogSize>,
    pub per_repo: SortedMap<RepoUiState>,
}

/// A dialog's size in CSS pixels after the user dragged its edges. It is applied as a minimum,
/// so the dialog never draws smaller than its content needs.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, Type)]
#[serde(default, rename_all = "camelCase")]
pub struct DialogSize {
    pub width: u32,
    pub height: u32,
    /// The dialog fills the whole window (its maximise button); the size above is kept for
    /// when it is restored.
    pub maximised: bool,
}

/// The Console header's Log button, which switches the other two off and shows the standard
/// output of the user's own commands as plain text, and its User and Internal buttons (which
/// kinds of command are listed). Log mode is the default.
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(default, rename_all = "camelCase")]
pub struct ConsoleView {
    pub user_action: bool,
    pub internal: bool,
    pub log: bool,
}

impl Default for ConsoleView {
    fn default() -> Self {
        Self { user_action: false, internal: false, log: true }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum FilesGroupBy {
    #[default]
    Status,
    Folder,
    /// One flat list.
    #[serde(rename = "none")]
    Flat,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum FilesSortBy {
    #[default]
    Name,
    Status,
    Modified,
}

/// The Files panel's grouping and sorting.
#[derive(Debug, Clone, Default, Serialize, Deserialize, Type)]
#[serde(default, rename_all = "camelCase")]
pub struct FilesView {
    pub group_by: FilesGroupBy,
    pub sort_by: FilesSortBy,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum DiffLayout {
    #[default]
    Unified,
    /// Side by side.
    Side,
}

/// The diff header's choices and the side-by-side divider.
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(default, rename_all = "camelCase")]
pub struct DiffView {
    pub layout: DiffLayout,
    /// Lines of context around each change; 99999 shows the whole file.
    pub context_lines: u32,
    pub ignore_whitespace: bool,
    pub ignore_cr_at_eol: bool,
    /// Share of the side-by-side width given to the old (left) side.
    pub split: f64,
}

impl Default for DiffView {
    fn default() -> Self {
        Self { layout: DiffLayout::Unified, context_lines: 3, ignore_whitespace: false, ignore_cr_at_eol: false, split: 0.5 }
    }
}

/// Sizes are percentages of the containing splitter group, matching what the
/// reka-ui `SplitterGroup` reports, so they survive window resizes.
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(default, rename_all = "camelCase")]
pub struct Panels {
    pub left_width: f64,
    pub repos_height: f64,
    pub files_width: f64,
    pub diff_height: f64,
    pub console_height: f64,
    pub console_collapsed: bool,
}

impl Default for Panels {
    fn default() -> Self {
        Self {
            left_width: 20.0,
            repos_height: 60.0,
            files_width: 35.0,
            diff_height: 45.0,
            console_height: 22.0,
            console_collapsed: false,
        }
    }
}

/// Pixel widths; the commit message column takes the remainder. Each is None
/// until the user drags its divider: the frontend then sizes the column to its
/// content (the graph to the lane count, the others to their widest value).
#[derive(Debug, Clone, Default, Serialize, Deserialize, Type)]
#[serde(default, rename_all = "camelCase")]
pub struct GraphColumns {
    pub graph: Option<u32>,
    #[serde(deserialize_with = "auto_if_old_default::<_, 140>")]
    #[specta(type = Option<u32>)]
    pub author: Option<u32>,
    #[serde(deserialize_with = "auto_if_old_default::<_, 150>")]
    #[specta(type = Option<u32>)]
    pub date: Option<u32>,
    /// The short commit ID column, at the right-hand end.
    #[serde(deserialize_with = "auto_if_old_default::<_, 76>")]
    #[specta(type = Option<u32>)]
    pub id: Option<u32>,
}

/// Before these columns sized themselves, their fixed defaults (140, 150, 76)
/// were always written to ui-state.json, so a saved default is read as "size
/// automatically". Any other saved width was dragged and stays.
fn auto_if_old_default<'de, D: Deserializer<'de>, const OLD: u32>(d: D) -> Result<Option<u32>, D::Error> {
    Ok(Option::<u32>::deserialize(d)?.filter(|w| *w != OLD))
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, Type)]
#[serde(default, rename_all = "camelCase")]
pub struct Session {
    /// Repository keys in tab order.
    pub open_tabs: Vec<String>,
    pub active_tab: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, Type)]
#[serde(default, rename_all = "camelCase")]
pub struct RepoUiState {
    pub last_selected_oid: Option<String>,
    /// Full names of the refs switched off in the commit graph.
    pub hidden_refs: Vec<String>,
    /// Colours chosen for refs in the commit graph, by full name: a hue, palette index 0 to 11.
    pub ref_colours: std::collections::BTreeMap<String, u8>,
    /// The mode last chosen in the Pull dialog. Until there is one, the dialog starts from the
    /// repository's Git config, then Merge.
    pub pull_mode: Option<crate::git::ops::PullMode>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_default_column_widths_read_as_automatic() {
        let old: GraphColumns = serde_json::from_str(r#"{"graph":null,"author":140,"date":150,"id":76}"#).unwrap();
        assert_eq!((old.author, old.date, old.id), (None, None, None));
        let dragged: GraphColumns = serde_json::from_str(r#"{"author":180,"date":95}"#).unwrap();
        assert_eq!((dragged.author, dragged.date, dragged.id), (Some(180), Some(95), None));
    }
}
