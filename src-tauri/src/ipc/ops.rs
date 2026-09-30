//! Write operations. Each takes the repository's exclusive lock, runs the
//! Git command, then refreshes refs, status and the graph so the UI reflects
//! the result at once. The watcher sees the same change a little later; its
//! refresh finds nothing new, so it neither rebuilds nor emits.

use std::time::Duration;

use tauri::{AppHandle, State};

use crate::error::{GitError, Result};
use crate::git::ops::{self, OpResult};
use crate::graph::loader::refresh_if_changed;
use crate::ipc::query::repo;
use crate::state::AppState;
use crate::transport::paths::NativePath;

/// How long to wait before retrying once when `index.lock` exists.
const INDEX_LOCK_RETRY: Duration = Duration::from_millis(250);

async fn after_write(app: &AppHandle, state: &AppState, key: &str) {
    if let Ok(h) = repo(state, key).await {
        let settings = state.settings();
        if let Err(e) = refresh_if_changed(Some(app), Some(&state.db), &h, settings.appearance.graph_options(), &settings.cache, true).await {
            tracing::warn!("refresh after write failed for {key}: {e}");
        }
    }
}

/// Run one operation under the exclusive lock. When Git reports that
/// `index.lock` exists, wait briefly and retry once (another Git process is
/// usually just finishing); if it is still there, return `IndexLocked` so the
/// UI can offer `op_remove_stale_lock`.
macro_rules! write_op {
    ($app:expr, $state:expr, $key:expr, |$client:ident| $body:expr) => {{
        let h = repo(&$state, &$key).await?;
        let out = {
            let _exclusive = h.lock.write().await;
            let $client = &h.client;
            let mut out = $body;
            if out.as_ref().is_ok_and(|r| ops::index_lock_in(r).is_some()) {
                tokio::time::sleep(INDEX_LOCK_RETRY).await;
                out = $body;
            }
            out
        };
        after_write(&$app, &$state, &$key).await;
        out.and_then(|r| match ops::index_lock_in(&r) {
            Some(lock_path) => Err(GitError::IndexLocked { lock_path }),
            None => Ok(r),
        })
    }};
}

/// Stage `paths` (additions, changes and deletions). Pass both paths of a rename.
#[tauri::command]
#[specta::specta]
pub async fn op_stage(app: AppHandle, state: State<'_, AppState>, key: String, paths: Vec<String>) -> Result<OpResult> {
    write_op!(app, state, key, |c| ops::stage(c, &paths).await)
}

/// Unstage `paths`, leaving the working tree as it is. Pass both paths of a staged rename.
#[tauri::command]
#[specta::specta]
pub async fn op_unstage(app: AppHandle, state: State<'_, AppState>, key: String, paths: Vec<String>) -> Result<OpResult> {
    write_op!(app, state, key, |c| ops::unstage(c, &paths).await)
}

/// Throw away changes: `tracked` back to the index (or to HEAD when `toHead`),
/// `untracked` deleted. Cannot be undone; confirm with the user first.
#[tauri::command]
#[specta::specta]
pub async fn op_discard(app: AppHandle, state: State<'_, AppState>, key: String, tracked: Vec<String>, untracked: Vec<String>, to_head: bool) -> Result<OpResult> {
    write_op!(app, state, key, |c| ops::discard(c, &tracked, &untracked, to_head).await)
}

/// Commit the index, or with `paths` stage exactly those paths and commit only
/// them (other staged paths stay staged). Pass both paths of a rename. An
/// empty `paths` with `amend` changes only the message. With
/// `amend` and an empty `message` the previous message is kept; fetch it with
/// `getCommitMessage(key, "HEAD")` to edit it instead.
#[tauri::command]
#[specta::specta]
pub async fn op_commit(app: AppHandle, state: State<'_, AppState>, key: String, message: String, paths: Option<Vec<String>>, amend: bool, sign_off: bool) -> Result<OpResult> {
    write_op!(app, state, key, |c| ops::commit(c, &message, paths.as_deref(), amend, sign_off).await)
}

/// Delete `<git dir>/index.lock` after a Git process crashed or was killed
/// and left it behind. The frontend confirms with the user first, since
/// deleting the lock of a Git process that is still running corrupts the
/// index. A lock that has already gone counts as success.
#[tauri::command]
#[specta::specta]
pub async fn op_remove_stale_lock(state: State<'_, AppState>, key: String) -> Result<()> {
    let h = repo(&state, &key).await?;
    let _exclusive = h.lock.write().await;
    let lock = NativePath(format!("{}/index.lock", h.info.git_dir.trim_end_matches(['/', '\\'])));
    tracing::info!("removing stale lock {} for {key}", lock.0);
    h.transport.remove_file(&lock).await
}

#[tauri::command]
#[specta::specta]
pub async fn op_checkout(app: AppHandle, state: State<'_, AppState>, key: String, target: String) -> Result<OpResult> {
    write_op!(app, state, key, |c| ops::checkout(c, &target).await)
}

#[tauri::command]
#[specta::specta]
pub async fn op_branch_create(app: AppHandle, state: State<'_, AppState>, key: String, name: String, start_point: String, checkout: bool) -> Result<OpResult> {
    write_op!(app, state, key, |c| ops::branch_create(c, &name, &start_point, checkout).await)
}

#[tauri::command]
#[specta::specta]
pub async fn op_branch_delete(app: AppHandle, state: State<'_, AppState>, key: String, name: String, force: bool) -> Result<OpResult> {
    write_op!(app, state, key, |c| ops::branch_delete(c, &name, force).await)
}

/// Merge `target` into the current branch. `conflicts` in the result means
/// the merge stopped and the working tree has unmerged paths.
#[tauri::command]
#[specta::specta]
pub async fn op_merge(app: AppHandle, state: State<'_, AppState>, key: String, target: String, no_ff: bool) -> Result<OpResult> {
    write_op!(app, state, key, |c| ops::merge(c, &target, no_ff).await)
}

#[tauri::command]
#[specta::specta]
pub async fn op_merge_abort(app: AppHandle, state: State<'_, AppState>, key: String) -> Result<OpResult> {
    write_op!(app, state, key, |c| ops::merge_abort(c).await)
}

#[tauri::command]
#[specta::specta]
pub async fn op_tag_create(app: AppHandle, state: State<'_, AppState>, key: String, name: String, target: String, message: Option<String>, force: bool) -> Result<OpResult> {
    write_op!(app, state, key, |c| ops::tag_create(c, &name, &target, message.as_deref(), force).await)
}

/// Save the working tree's changes as a new stash. `message` names it; without one Git names it
/// after the commit it was made on.
#[tauri::command]
#[specta::specta]
pub async fn op_stash_push(app: AppHandle, state: State<'_, AppState>, key: String, message: Option<String>, include_untracked: bool) -> Result<OpResult> {
    write_op!(app, state, key, |c| ops::stash_push(c, message.as_deref(), include_untracked).await)
}

/// Apply the stash at `index` and keep it. `oid` is the stash's commit as the list showed it;
/// when the index no longer points at it, nothing runs.
#[tauri::command]
#[specta::specta]
pub async fn op_stash_apply(app: AppHandle, state: State<'_, AppState>, key: String, index: u32, oid: String) -> Result<OpResult> {
    write_op!(app, state, key, |c| ops::stash_apply(c, index, &oid).await)
}

/// Apply the stash at `index`, then drop it; Git keeps it when applying conflicts.
#[tauri::command]
#[specta::specta]
pub async fn op_stash_pop(app: AppHandle, state: State<'_, AppState>, key: String, index: u32, oid: String) -> Result<OpResult> {
    write_op!(app, state, key, |c| ops::stash_pop(c, index, &oid).await)
}

/// Delete the stash at `index` without applying it.
#[tauri::command]
#[specta::specta]
pub async fn op_stash_drop(app: AppHandle, state: State<'_, AppState>, key: String, index: u32, oid: String) -> Result<OpResult> {
    write_op!(app, state, key, |c| ops::stash_drop(c, index, &oid).await)
}

#[tauri::command]
#[specta::specta]
pub async fn op_tag_delete(app: AppHandle, state: State<'_, AppState>, key: String, name: String) -> Result<OpResult> {
    write_op!(app, state, key, |c| ops::tag_delete(c, &name).await)
}
