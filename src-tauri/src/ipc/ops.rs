//! Write operations. Each takes the repository's exclusive lock, runs the
//! Git command, then refreshes refs, status and the graph so the UI reflects
//! the result at once. The watcher sees the same change a little later; its
//! refresh finds nothing new, so it neither rebuilds nor emits.

use std::time::Duration;

use tauri::{AppHandle, State};
use tauri_specta::Event;
use tokio_util::sync::CancellationToken;

use crate::error::{GitError, Result};
use crate::git::ops::{self, OpResult, PullMode, PushTarget};
use crate::graph::loader::refresh_if_changed;
use crate::ipc::events::OperationProgress;
use crate::ipc::query::repo;
use crate::repo::RepoHandle;
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
            let _gate = h.write_gate.lock().await;
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
pub async fn op_rebase_abort(app: AppHandle, state: State<'_, AppState>, key: String) -> Result<OpResult> {
    write_op!(app, state, key, |c| ops::rebase_abort(c).await)
}

/// Pull `branch` from `remote` into the current branch, as `git fetch` and then `git merge` or
/// `git rebase` (see `ops::pull_fetch` and `ops::pull_integrate`). `opId` is chosen by the
/// caller: each progress update arrives as an `operationProgress` event carrying it, and
/// `opCancel` with it stops the pull while it is fetching, which then fails with `cancelled` and
/// changes no branch. Once the fetch is done, an event with `cancellable: false` says the local
/// step has begun, which always runs to the end. `autostash` stashes uncommitted changes first
/// and reapplies them afterwards.
///
/// Holds the write gate throughout, so no other write overlaps, but takes the exclusive lock only
/// for the local step: reads (status, graph pages, diffs) carry on during the network transfer.
#[tauri::command]
#[specta::specta]
#[allow(clippy::too_many_arguments)]
pub async fn op_pull(
    app: AppHandle,
    state: State<'_, AppState>,
    key: String,
    op_id: String,
    remote: String,
    branch: String,
    mode: PullMode,
    autostash: bool,
) -> Result<OpResult> {
    let h = repo(&state, &key).await?;
    let emit = progress_emitter(&app, &key, &op_id);
    let out = {
        let op = h.start_operation(&op_id)?;
        let _gate = gate_or_cancel(&h, &op.token).await?;
        let progress = |line: String| emit(line, true);
        let fetched = ops::pull_fetch(&h.client, &remote, &branch, &progress, &op.token).await;
        // From here on Stop no longer applies: `opCancel` finds no operation and resolves false.
        let stopped = op.token.is_cancelled();
        drop(op);
        match fetched {
            // Stop pressed just as the fetch finished: honour it before anything is merged.
            Ok(_) if stopped => Err(GitError::Cancelled),
            Ok(f) if f.ok => {
                emit(
                    match mode {
                        PullMode::Rebase => format!("Rebasing onto {remote}/{branch}…"),
                        PullMode::Merge => format!("Merging {remote}/{branch}…"),
                        PullMode::FastForwardOnly => format!("Fast-forwarding to {remote}/{branch}…"),
                    },
                    false,
                );
                let _exclusive = h.lock.write().await;
                ops::pull_integrate(&h.client, &remote, &branch, mode, autostash).await.map(|r| ops::combine(f, r))
            }
            other => other,
        }
    };
    after_write(&app, &state, &key).await;
    out.and_then(|r| match ops::index_lock_in(&r) {
        Some(lock_path) => Err(GitError::IndexLocked { lock_path }),
        None => Ok(r),
    })
}

/// Sends a running operation's progress as `operationProgress` events.
fn progress_emitter<'a>(app: &'a AppHandle, key: &'a str, op_id: &'a str) -> impl Fn(String, bool) + Send + Sync + 'a {
    move |line, cancellable| {
        let _ = OperationProgress { repo_id: key.to_string(), op_id: op_id.to_string(), line, cancellable }.emit(app);
    }
}

/// Wait for the write gate. Cancelling while another write still holds it stops the wait.
async fn gate_or_cancel<'a>(h: &'a RepoHandle, token: &CancellationToken) -> Result<tokio::sync::MutexGuard<'a, ()>> {
    tokio::select! {
        biased;
        _ = token.cancelled() => Err(GitError::Cancelled),
        g = h.write_gate.lock() => Ok(g),
    }
}

/// Run a network operation that is cancellable from start to end (fetch, push): registered
/// under `op_id`, under the write gate but not the exclusive lock, with progress as
/// `operationProgress` events and a refresh afterwards. A macro rather than a function taking an
/// async closure, which Tauri's `Send` bound on commands cannot see through.
macro_rules! network_op {
    ($app:expr, $state:expr, $key:expr, $op_id:expr, |$h:ident, $progress:ident, $cancel:ident| $body:expr) => {{
        let $h = repo(&$state, &$key).await?;
        let emit = progress_emitter(&$app, &$key, &$op_id);
        let out = {
            let op = $h.start_operation(&$op_id)?;
            let _gate = gate_or_cancel(&$h, &op.token).await?;
            let $progress = |line: String| emit(line, true);
            let $cancel = &op.token;
            $body
        };
        after_write(&$app, &$state, &$key).await;
        out
    }};
}

/// Fetch from every remote (`git fetch --all`). `opId` works as for `opPull`; `opCancel` stops
/// the fetch at any point, leaving the remote-tracking branches where they were.
#[tauri::command]
#[specta::specta]
pub async fn op_fetch(app: AppHandle, state: State<'_, AppState>, key: String, op_id: String) -> Result<OpResult> {
    network_op!(app, state, key, op_id, |h, progress, cancel| ops::fetch_all(&h.client, &progress, cancel).await)
}

/// Push local `branch`. With no `target`, to the upstream it tracks (failing with `invalidPath`
/// when it has none: call `getUpstream` first); with a `target`, there, and `setUpstream` makes
/// the branch track it. Never forces: a rejected push comes back with `ok: false` and
/// `[rejected]` in stderr. `opId` works as for `opPull`; `opCancel` stops it at any point.
#[tauri::command]
#[specta::specta]
#[allow(clippy::too_many_arguments)]
pub async fn op_push(
    app: AppHandle,
    state: State<'_, AppState>,
    key: String,
    op_id: String,
    branch: String,
    target: Option<PushTarget>,
    set_upstream: bool,
) -> Result<OpResult> {
    network_op!(app, state, key, op_id, |h, progress, cancel| {
        let resolved = match target {
            Some(t) => Ok((t, set_upstream)),
            None => match ops::upstream_of(&h.client, &branch).await {
                Ok(Some(t)) => Ok((t, false)),
                Ok(None) => Err(GitError::InvalidPath { path: branch.clone(), reason: "the branch does not track a remote branch; choose where to push it".into() }),
                Err(e) => Err(e),
            },
        };
        match resolved {
            Ok((target, set_upstream)) => ops::push(&h.client, &branch, &target, set_upstream, &progress, cancel).await,
            Err(e) => Err(e),
        }
    })
}

/// The remote branch local `branch` tracks (`branch.<branch>.remote` and `.merge`), or null.
#[tauri::command]
#[specta::specta]
pub async fn get_upstream(state: State<'_, AppState>, key: String, branch: String) -> Result<Option<PushTarget>> {
    let h = repo(&state, &key).await?;
    let _shared = h.lock.read().await;
    ops::upstream_of(&h.client, &branch).await
}

/// Stop a running operation started with `opId` (a pull while it fetches). Resolves false when
/// it has finished or reached a step that cannot be stopped. The pull's own command returns once
/// Git has exited, with `cancelled`.
#[tauri::command]
#[specta::specta]
pub async fn op_cancel(state: State<'_, AppState>, key: String, op_id: String) -> Result<bool> {
    let h = repo(&state, &key).await?;
    Ok(h.cancel_operation(&op_id))
}

/// The pull mode the repository's Git config asks for when pulling into `branch`
/// (`branch.<branch>.rebase`, `pull.rebase`, `pull.ff`), or null when none is set.
#[tauri::command]
#[specta::specta]
pub async fn get_pull_mode(state: State<'_, AppState>, key: String, branch: Option<String>) -> Result<Option<PullMode>> {
    let h = repo(&state, &key).await?;
    let _shared = h.lock.read().await;
    ops::configured_pull_mode(&h.client, branch.as_deref()).await
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
