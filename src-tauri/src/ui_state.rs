//! `ui-state.json`: window geometry, panel sizes, column widths, open tabs.
//!
//! Written constantly and never hand-edited; safe to delete. Kept out of
//! `settings.json` so the churn never races a hand edit there.

use serde::{Deserialize, Deserializer, Serialize};
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
