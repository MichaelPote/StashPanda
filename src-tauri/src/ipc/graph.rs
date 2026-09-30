//! Commit graph commands: open (kick off a load), page rows, map oids to rows.

use tauri::{AppHandle, State};

use crate::error::{GitError, Result};
use std::sync::Arc;

use crate::graph::loader::{open_pipeline, rebuild_filtered, refresh_full, refresh_status_only, SnapshotReason};
use crate::graph::snapshot::{GraphMeta, GraphRow};
use crate::ipc::query::repo;
use crate::state::AppState;

fn empty_meta() -> GraphMeta {
    GraphMeta {
        version: 0,
        total: 0,
        complete: false,
        truncated: false,
        max_lanes: 0,
        open_lanes: 0,
        head: None,
        head_ref: None,
        worktree: None,
        ref_colours: Default::default(),
    }
}

/// Switch refs (full names) off in the commit graph, or back on by leaving them out, and
/// rebuild the graph from the commits already loaded; no Git command runs. The frontend saves
/// the same list in `ui-state.json` (`perRepo[key].hiddenRefs`), which `repo_open` reads.
#[tauri::command]
#[specta::specta]
pub async fn graph_set_hidden_refs(app: AppHandle, state: State<'_, AppState>, key: String, hidden: Vec<String>) -> Result<GraphMeta> {
    let h = repo(&state, &key).await?;
    *h.hidden.write().expect("hidden refs lock") = Arc::new(hidden.into_iter().collect());
    let order = state.settings().appearance.graph_order;
    Ok(rebuild_filtered(Some(&app), &h, order).map(|s| s.meta()).unwrap_or_else(empty_meta))
}

/// Current snapshot metadata. When none exists yet a live load starts in the
/// background and `graph_snapshot` fires when it lands.
#[tauri::command]
#[specta::specta]
pub async fn graph_open(app: AppHandle, state: State<'_, AppState>, key: String) -> Result<GraphMeta> {
    let h = repo(&state, &key).await?;
    if let Some(snap) = h.graph.load_full() {
        return Ok(snap.meta());
    }
    let settings = state.settings();
    let handle = h.clone();
    let app2 = app.clone();
    let db = state.db.clone();
    tauri::async_runtime::spawn(async move {
        open_pipeline(&app2, &db, &handle, settings.appearance.graph_order, &settings.cache).await;
    });
    Ok(empty_meta())
}

#[tauri::command]
#[specta::specta]
pub async fn graph_meta(state: State<'_, AppState>, key: String) -> Result<GraphMeta> {
    let h = repo(&state, &key).await?;
    Ok(h.graph.load_full().map(|s| s.meta()).unwrap_or_else(empty_meta))
}

#[tauri::command]
#[specta::specta]
pub async fn graph_rows(
    state: State<'_, AppState>,
    key: String,
    version: u64,
    start: u32,
    count: u32,
) -> Result<Vec<GraphRow>> {
    let h = repo(&state, &key).await?;
    let snap = h.graph.load_full().ok_or(GitError::StaleVersion { requested: version, current: 0 })?;
    if snap.version != version {
        return Err(GitError::StaleVersion { requested: version, current: snap.version });
    }
    Ok(snap.rows(start, count.min(1000)))
}

#[tauri::command]
#[specta::specta]
pub async fn graph_find(
    state: State<'_, AppState>,
    key: String,
    version: u64,
    oids: Vec<String>,
) -> Result<Vec<Option<u32>>> {
    let h = repo(&state, &key).await?;
    let snap = h.graph.load_full().ok_or(GitError::StaleVersion { requested: version, current: 0 })?;
    if snap.version != version {
        return Err(GitError::StaleVersion { requested: version, current: snap.version });
    }
    Ok(oids.iter().map(|o| snap.find(o)).collect())
}

/// Reload refs and history (full) or just the working tree row (status only).
#[tauri::command]
#[specta::specta]
pub async fn graph_refresh(app: AppHandle, state: State<'_, AppState>, key: String, full: bool) -> Result<GraphMeta> {
    let h = repo(&state, &key).await?;
    let settings = state.settings();
    let snap = if full {
        refresh_full(Some(&app), Some(&state.db), &h, settings.appearance.graph_order, &settings.cache, SnapshotReason::Refresh).await?
    } else {
        refresh_status_only(Some(&app), &h, settings.appearance.graph_order, &settings.cache).await?
    };
    Ok(snap.meta())
}
