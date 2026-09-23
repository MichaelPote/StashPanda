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
        if let Err(e) = refresh_if_changed(Some(app), Some(&state.db), &h, settings.appearance.graph_order, &settings.cache, true).await {
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

#[tauri::command]
#[specta::specta]
pub async fn op_tag_delete(app: AppHandle, state: State<'_, AppState>, key: String, name: String) -> Result<OpResult> {
    write_op!(app, state, key, |c| ops::tag_delete(c, &name).await)
}
