//! `ui-state.json`: window geometry, panel sizes, column widths, open tabs.
//!
//! Written constantly and never hand-edited; safe to delete. Kept out of
//! `settings.json` so the churn never races a hand edit there.

use serde::{Deserialize, Serialize};
use specta::Type;

use crate::settings::SortedMap;

#[derive(Debug, Clone, Default, Serialize, Deserialize, Type)]
#[serde(default, rename_all = "camelCase")]
pub struct UiState {
    pub panels: Panels,
    pub graph_columns: GraphColumns,
    pub session: Session,
    pub per_repo: SortedMap<RepoUiState>,
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

/// Pixel widths; the subject column takes the remainder. `graph` is None
/// until the user drags the handle: the frontend then sizes the column to the
/// lane count of the current snapshot.
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(default, rename_all = "camelCase")]
pub struct GraphColumns {
    pub graph: Option<u32>,
    pub author: u32,
    pub date: u32,
    /// The short commit ID column, at the right-hand end.
    pub id: u32,
}

impl Default for GraphColumns {
    fn default() -> Self {
        Self {
            graph: None,
            author: 140,
            date: 150,
            id: 76,
        }
    }
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
}
