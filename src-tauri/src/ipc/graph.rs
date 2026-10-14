//! Commit graph commands: open (kick off a load), page rows, map oids to rows.

use tauri::{AppHandle, State};

use crate::error::{GitError, Result};
use std::sync::Arc;

use crate::graph::loader::{open_pipeline, rebuild_filtered, refresh_full, refresh_status_only, SnapshotReason};
use crate::graph::snapshot::{GraphMeta, GraphRow, GraphSearchResult, SearchDirection};
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
        widest_authors: Vec::new(),
    }
}

/// Switch refs (full names) off in the commit graph, or back on by leaving them out, and
/// rebuild the graph from the commits already loaded; no Git command runs. The frontend saves
/// the same list in the UI state (`perRepo[key].hiddenRefs`, stored in the database), which `repo_open` reads.
#[tauri::command]
#[specta::specta]
pub async fn graph_set_hidden_refs(app: AppHandle, state: State<'_, AppState>, key: String, hidden: Vec<String>) -> Result<GraphMeta> {
    let h = repo(&state, &key).await?;
    *h.hidden.write().expect("hidden refs lock") = Arc::new(hidden.into_iter().collect());
    let options = state.settings().appearance.graph_options();
    Ok(rebuild_filtered(Some(&app), &h, options).map(|s| s.meta()).unwrap_or_else(empty_meta))
}

/// Sets the colours chosen for refs (full name to hue, palette index 0 to 5) and rebuilds the
/// graph with them; see `GraphSnapshot::build_coloured`. The frontend keeps the same map in the
/// UI state (`perRepo[key].refColours`, stored in the database), which `repo_open` reads.
#[tauri::command]
#[specta::specta]
pub async fn graph_set_ref_colours(
    app: AppHandle,
    state: State<'_, AppState>,
    key: String,
    colours: std::collections::BTreeMap<String, u8>,
) -> Result<GraphMeta> {
    let h = repo(&state, &key).await?;
    *h.colours.write().expect("ref colours lock") = Arc::new(colours.into_iter().collect());
    let options = state.settings().appearance.graph_options();
    Ok(rebuild_filtered(Some(&app), &h, options).map(|s| s.meta()).unwrap_or_else(empty_meta))
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
        open_pipeline(&app2, &db, &handle, settings.appearance.graph_options(), &settings.cache).await;
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

/// The graph's Find box: the next row from `from_row` in `direction` whose subject, author or
/// oid matches `query`, wrapping around the ends, and how many rows match in all.
#[tauri::command]
#[specta::specta]
pub async fn graph_search(
    state: State<'_, AppState>,
    key: String,
    version: u64,
    query: String,
    from_row: Option<u32>,
    direction: SearchDirection,
) -> Result<GraphSearchResult> {
    let h = repo(&state, &key).await?;
    let snap = h.graph.load_full().ok_or(GitError::StaleVersion { requested: version, current: 0 })?;
    if snap.version != version {
        return Err(GitError::StaleVersion { requested: version, current: snap.version });
    }
    // About 10 ms for 85,000 commits in a release build; keep it off the async workers anyway.
    tokio::task::spawn_blocking(move || snap.search(&query, from_row, direction))
        .await
        .map_err(|e| GitError::internal(e.to_string()))
}

/// Reload refs and history (full) or just the working tree row (status only). The full reload
/// reads the whole history again rather than only what changed, so it also repairs a cache that
/// has gone wrong.
#[tauri::command]
#[specta::specta]
pub async fn graph_refresh(app: AppHandle, state: State<'_, AppState>, key: String, full: bool) -> Result<GraphMeta> {
    let h = repo(&state, &key).await?;
    let settings = state.settings();
    let snap = if full {
        refresh_full(Some(&app), Some(&state.db), &h, settings.appearance.graph_options(), &settings.cache, SnapshotReason::Refresh, true).await?
    } else {
        refresh_status_only(Some(&app), &h, settings.appearance.graph_options(), &settings.cache).await?
    };
    Ok(snap.meta())
}
