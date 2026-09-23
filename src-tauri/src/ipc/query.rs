//! Read-only queries against an open repository.

use std::sync::Arc;

use tauri::State;

use crate::error::{GitError, Result};
use crate::git::queries::blob::{get_blob, get_worktree_file, BlobContent};
use crate::git::queries::commit_files::{get_commit_files, FileChange};
use crate::git::queries::diff::{get_diff, DiffOptions, DiffTarget, DiffText};
use crate::git::queries::log::{get_commit_body, get_stashes, StashEntry};
use crate::git::queries::refs::{get_refs, RefsSnapshot};
use crate::git::queries::status::{get_status, StatusSnapshot};
use crate::repo::RepoHandle;
use crate::state::AppState;

/// Look up an open repository and take a read slot on it.
pub async fn repo(state: &AppState, key: &str) -> Result<Arc<RepoHandle>> {
    state
        .repos
        .read()
        .await
        .get(key)
        .cloned()
        .ok_or_else(|| GitError::RepoNotOpen { repo_id: key.to_string() })
}

macro_rules! with_read_slot {
    ($handle:expr, $body:expr) => {{
        let _shared = $handle.lock.read().await;
        let _slot = $handle
            .read_slots
            .acquire()
            .await
            .map_err(|_| GitError::Cancelled)?;
        $body
    }};
}

#[tauri::command]
#[specta::specta]
pub async fn get_repo_status(state: State<'_, AppState>, key: String) -> Result<StatusSnapshot> {
    let h = repo(&state, &key).await?;
    with_read_slot!(h, get_status(&h.client).await)
}

#[tauri::command]
#[specta::specta]
pub async fn get_repo_refs(state: State<'_, AppState>, key: String) -> Result<RefsSnapshot> {
    let h = repo(&state, &key).await?;
    with_read_slot!(h, get_refs(&h.client).await)
}

#[tauri::command]
#[specta::specta]
pub async fn get_commit_changes(
    state: State<'_, AppState>,
    key: String,
    commit: String,
    parent: Option<String>,
) -> Result<Vec<FileChange>> {
    let h = repo(&state, &key).await?;
    with_read_slot!(h, get_commit_files(&h.client, &commit, parent.as_deref()).await)
}

#[tauri::command]
#[specta::specta]
pub async fn get_commit_message(state: State<'_, AppState>, key: String, commit: String) -> Result<String> {
    let h = repo(&state, &key).await?;
    with_read_slot!(h, get_commit_body(&h.client, &commit).await)
}

#[tauri::command]
#[specta::specta]
pub async fn get_file_diff(
    state: State<'_, AppState>,
    key: String,
    target: DiffTarget,
    path: String,
    old_path: Option<String>,
    options: Option<DiffOptions>,
) -> Result<DiffText> {
    let h = repo(&state, &key).await?;
    let opts = options.unwrap_or_default();
    with_read_slot!(h, get_diff(&h.client, &target, &path, old_path.as_deref(), &opts).await)
}

#[tauri::command]
#[specta::specta]
pub async fn get_blob_content(state: State<'_, AppState>, key: String, spec: String) -> Result<BlobContent> {
    let h = repo(&state, &key).await?;
    with_read_slot!(h, get_blob(&h.client, &spec).await)
}

#[tauri::command]
#[specta::specta]
pub async fn get_worktree_file_content(state: State<'_, AppState>, key: String, path: String) -> Result<BlobContent> {
    let h = repo(&state, &key).await?;
    with_read_slot!(h, get_worktree_file(&h.client, &path).await)
}

#[tauri::command]
#[specta::specta]
pub async fn get_repo_stashes(state: State<'_, AppState>, key: String) -> Result<Vec<StashEntry>> {
    let h = repo(&state, &key).await?;
    with_read_slot!(h, get_stashes(&h.client).await)
}
