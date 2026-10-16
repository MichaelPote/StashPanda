//! Write operations. Each takes the repository's exclusive lock, runs the
//! Git command, then refreshes refs, status and the graph so the UI reflects
//! the result at once. The watcher sees the same change a little later; its
//! refresh finds nothing new, so it neither rebuilds nor emits.

use std::time::Duration;

use tauri::{AppHandle, State};
use tauri_specta::Event;
use tokio_util::sync::CancellationToken;

use crate::error::{GitError, Result};
use crate::git::ops::{self, OpResult, PullMode, PushTarget, ResetMode, ResolveSide, SyncResult};
use crate::git::queries::in_progress::InProgressKind;
use crate::git::ops::lines::{LineAction, SelectedLine};
use crate::graph::loader::refresh_if_changed;
use crate::ipc::events::OperationProgress;
use crate::ipc::query::repo;
use crate::repo::{OperationGuard, RepoHandle};
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
/// `index.lock` or `config.lock` exists, wait briefly and retry once (another
/// Git process is usually just finishing); if it is still there, return
/// `IndexLocked` or `ConfigLocked` so the UI can offer `op_remove_stale_lock`.
macro_rules! write_op {
    ($app:expr, $state:expr, $key:expr, |$client:ident| $body:expr) => {{
        let h = repo(&$state, &$key).await?;
        let out = {
            let _gate = h.write_gate.lock().await;
            let _exclusive = h.lock.write().await;
            let $client = &h.client;
            let mut out = $body;
            if out.as_ref().is_ok_and(|r| ops::lock_in(r).is_some()) {
                tokio::time::sleep(INDEX_LOCK_RETRY).await;
                out = $body;
            }
            out
        };
        after_write(&$app, &$state, &$key).await;
        out.and_then(|r| match ops::lock_in(&r) {
            Some(locked) => Err(locked),
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

/// Stage, unstage or discard some lines of `path`: the lines the diff viewer showed as selected,
/// each named by its number (in the new file for an addition, the old file for a removal) and
/// its text. Stage and discard work on the unstaged diff, unstage on the staged one; `untracked`
/// says the file is not in the index yet. Fails with `invalidPath` when the file has changed
/// since the diff was shown, or when the lines cannot be changed without a neighbouring line.
/// Discard cannot be undone; confirm with the user first.
#[tauri::command]
#[specta::specta]
pub async fn op_apply_lines(app: AppHandle, state: State<'_, AppState>, key: String, action: LineAction, path: String, untracked: bool, lines: Vec<SelectedLine>) -> Result<OpResult> {
    write_op!(app, state, key, |c| ops::lines::apply_lines(c, action, &path, untracked, &lines).await)
}

/// Commit the index, or with `paths` stage exactly those paths and commit only
/// them (other staged paths stay staged). When every one of `paths` is
/// already staged, they are committed as staged, unstaged changes left out.
/// Pass both paths of a rename. An empty `paths` with `amend` changes only
/// the message. With `amend` and an empty `message` the previous message is
/// kept; fetch it with `getCommitMessage(key, "HEAD")` to edit it instead.
#[tauri::command]
#[specta::specta]
pub async fn op_commit(app: AppHandle, state: State<'_, AppState>, key: String, message: String, paths: Option<Vec<String>>, amend: bool, sign_off: bool) -> Result<OpResult> {
    write_op!(app, state, key, |c| ops::commit(c, &message, paths.as_deref(), amend, sign_off).await)
}

/// Which lock file `op_remove_stale_lock` deletes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub enum StaleLock {
    /// `<git dir>/index.lock`, after an `indexLocked` error.
    Index,
    /// `config.lock` next to the repository's `config` (in the main repository's Git folder,
    /// for a linked worktree), after a `configLocked` error.
    Config,
}

/// Delete a lock file a Git process left behind when it crashed or was killed:
/// `index.lock`, or `config.lock`. The frontend confirms with the user first,
/// since deleting the lock of a Git process that is still running loses what
/// it was writing. A lock that has already gone counts as success.
#[tauri::command]
#[specta::specta]
pub async fn op_remove_stale_lock(state: State<'_, AppState>, key: String, lock: StaleLock) -> Result<()> {
    let h = repo(&state, &key).await?;
    let _exclusive = h.lock.write().await;
    let dir = match lock {
        StaleLock::Index => &h.info.git_dir,
        StaleLock::Config if !h.info.common_dir.is_empty() => &h.info.common_dir,
        StaleLock::Config => &h.info.git_dir,
    };
    let file = if lock == StaleLock::Index { "index.lock" } else { "config.lock" };
    let lock = NativePath(format!("{}/{file}", dir.trim_end_matches(['/', '\\'])));
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

/// Rebase the current branch onto `onto` (a branch name or commit), not interactively.
/// `conflicts` in the result means the rebase stopped on a commit and is still open: the status's
/// `inProgress` then describes it, and `opContinue`, `opSkip` or `opRebaseAbort` go on.
#[tauri::command]
#[specta::specta]
pub async fn op_rebase(app: AppHandle, state: State<'_, AppState>, key: String, onto: String) -> Result<OpResult> {
    write_op!(app, state, key, |c| ops::rebase(c, &onto).await)
}

/// Finish the merge, cherry-pick or revert in progress, or go on with the rebase, once no file is
/// conflicted: `git <command> --continue`, keeping the message Git prepared. A rebase that stops
/// on the next commit's conflicts reports `conflicts` again.
#[tauri::command]
#[specta::specta]
pub async fn op_continue(app: AppHandle, state: State<'_, AppState>, key: String, kind: InProgressKind) -> Result<OpResult> {
    write_op!(app, state, key, |c| ops::continue_operation(c, kind).await)
}

/// Leave out the commit the rebase or cherry-pick in progress stopped at, and go on with the next
/// one (`--skip`). Fails with `invalidPath` for a merge or revert.
#[tauri::command]
#[specta::specta]
pub async fn op_skip(app: AppHandle, state: State<'_, AppState>, key: String, kind: InProgressKind) -> Result<OpResult> {
    write_op!(app, state, key, |c| ops::skip_operation(c, kind).await)
}

/// Resolve conflicted `paths` by keeping one side's whole version (`ours` is the branch being
/// merged into, `theirs` what comes in; a rebase swaps the two, see `ResolveSide`), and mark them
/// resolved. A side that deleted the file deletes it. Fails with `invalidPath` when a path is not
/// conflicted any more. To mark a file resolved as it is on disk, stage it with `opStage`.
#[tauri::command]
#[specta::specta]
pub async fn op_resolve_conflicts(app: AppHandle, state: State<'_, AppState>, key: String, paths: Vec<String>, side: ResolveSide) -> Result<OpResult> {
    write_op!(app, state, key, |c| ops::resolve_conflicts(c, &paths, side).await)
}

/// Write Git's merge of conflicted `path` to the working tree again, with `diff3` conflict markers
/// that carry each block's base text (`git checkout --conflict=diff3`): the merge editor's "Start
/// again from Git's merge". Throws away what has been done to the file; confirm with the user
/// first. Fails with `invalidPath` when the file is not conflicted any more.
#[tauri::command]
#[specta::specta]
pub async fn op_restore_conflict(app: AppHandle, state: State<'_, AppState>, key: String, path: String) -> Result<OpResult> {
    write_op!(app, state, key, |c| ops::restore_conflict(c, &path).await)
}

/// Save `content` as working tree file `path`, byte for byte as UTF-8: the caller puts back the
/// file's line endings and byte order mark. `expectedHash` is the `hash` from `getConflictFile`;
/// when the file on disk has changed since, nothing is written and this fails with
/// `changedOnDisk`. Returns the new hash, for the next save.
#[tauri::command]
#[specta::specta]
pub async fn write_worktree_file(app: AppHandle, state: State<'_, AppState>, key: String, path: String, content: String, expected_hash: String) -> Result<String> {
    let h = repo(&state, &key).await?;
    let out = {
        let _gate = h.write_gate.lock().await;
        let _exclusive = h.lock.write().await;
        ops::write_worktree_file(&h.client, &path, &content, &expected_hash).await
    };
    after_write(&app, &state, &key).await;
    out
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
        pull_steps(&h, &op, &emit, &remote, &branch, mode, autostash).await
    };
    after_write(&app, &state, &key).await;
    out.and_then(|r| match ops::index_lock_in(&r) {
        Some(lock_path) => Err(GitError::IndexLocked { lock_path }),
        None => Ok(r),
    })
}

/// A pull's two steps, for a caller holding the write gate: the fetch, which `op` can stop, then
/// the merge or rebase under the exclusive lock, during which it cannot. Leaves `op`
/// uncancellable.
#[allow(clippy::too_many_arguments)]
async fn pull_steps(
    h: &RepoHandle,
    op: &OperationGuard<'_>,
    emit: &(dyn Fn(String, bool) + Send + Sync),
    remote: &str,
    branch: &str,
    mode: PullMode,
    autostash: bool,
) -> Result<OpResult> {
    let progress = |line: String| emit(line, true);
    let fetched = ops::pull_fetch(&h.client, remote, branch, &progress, &op.token).await;
    // From here on Stop no longer applies: `opCancel` resolves false.
    op.set_cancellable(false);
    match fetched {
        // Stop pressed just as the fetch finished: honour it before anything is merged.
        Ok(_) if op.token.is_cancelled() => Err(GitError::Cancelled),
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
            ops::pull_integrate(&h.client, remote, branch, mode, autostash).await.map(|r| ops::combine(f, r))
        }
        other => other,
    }
}

/// Pull local `branch`'s upstream into it, then push it back there: one operation under the write
/// gate, so no other write runs in between. The pull works as for `opPull`. The push runs only
/// when the pull succeeded (an autostash that conflicted on the way back still counts, since the
/// commits are complete) and the branch has commits the upstream lacks. `opCancel` stops it while
/// it fetches or pushes, not while it merges or rebases; a stop after the merge comes back as
/// `pushStopped` rather than `cancelled`, since the pull did happen. Fails with `invalidPath`
/// when the branch has no upstream, and, before anything runs, when `git push` would not push to
/// its upstream because the names differ (see `ops::push_target_for`). A `pushTo` the user chose
/// for this sync replaces where `git push` would push.
#[tauri::command]
#[specta::specta]
#[allow(clippy::too_many_arguments)]
pub async fn op_sync(
    app: AppHandle,
    state: State<'_, AppState>,
    key: String,
    op_id: String,
    branch: String,
    mode: PullMode,
    autostash: bool,
    push_to: Option<PushTarget>,
) -> Result<SyncResult> {
    let h = repo(&state, &key).await?;
    let emit = progress_emitter(&app, &key, &op_id);
    let out = sync_steps(&h, &op_id, &emit, &branch, mode, autostash, push_to).await;
    after_write(&app, &state, &key).await;
    out
}

/// The whole of `op_sync` except the refresh afterwards.
#[allow(clippy::too_many_arguments)]
async fn sync_steps(
    h: &RepoHandle,
    op_id: &str,
    emit: &(dyn Fn(String, bool) + Send + Sync),
    branch: &str,
    mode: PullMode,
    autostash: bool,
    push_to: Option<PushTarget>,
) -> Result<SyncResult> {
    let op = h.start_operation(op_id)?;
    let _gate = gate_or_cancel(h, &op.token).await?;
    let target = ops::upstream_of(&h.client, branch).await?.ok_or_else(|| GitError::InvalidPath {
        path: branch.to_string(),
        reason: "the branch does not track a remote branch, so there is nothing to sync with".into(),
    })?;
    let push_target = match push_to {
        Some(chosen) => chosen,
        None => ops::push_target_for(&h.client, branch, target.clone()).await?,
    };
    let pull = pull_steps(h, &op, emit, &target.remote, &target.branch, mode, autostash).await?;
    if let Some(lock_path) = ops::index_lock_in(&pull) {
        return Err(GitError::IndexLocked { lock_path });
    }
    let mut result = SyncResult { target: target.clone(), push_target: push_target.clone(), pull, push: None, nothing_to_push: false, push_stopped: false };
    if !result.pull.ok {
        return Ok(result);
    }
    // `FETCH_HEAD` is the upstream as just fetched, whatever remote-tracking branch, if any, the
    // remote's fetch refspec keeps it in. Pushing under another name always runs.
    if push_target == target && ops::commits_ahead(&h.client, "FETCH_HEAD").await? == 0 {
        result.nothing_to_push = true;
        return Ok(result);
    }
    op.set_cancellable(true);
    emit(format!("Pushing to {}/{}…", push_target.remote, push_target.branch), true);
    // A stop pressed during the merge or rebase was refused, so only one pressed since counts.
    if op.token.is_cancelled() {
        result.push_stopped = true;
        return Ok(result);
    }
    let progress = |line: String| emit(line, true);
    match ops::push(&h.client, branch, &push_target, &progress, &op.token).await {
        Ok(r) => result.push = Some(r),
        Err(GitError::Cancelled) => result.push_stopped = true,
        Err(e) => return Err(e),
    }
    Ok(result)
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

/// Push local `branch`. With no `target`, where plain `git push` would push it (failing with
/// `invalidPath` when it has no upstream, or when `push.default` refuses an upstream with another
/// name: call `getPushTarget` first); with a `target` the user chose, there. It never sets the
/// upstream; to make the branch track the target, call `opBranchSetUpstream` after it succeeds,
/// as its own step that can be retried if `.git/config` is locked. Never forces: a rejected push comes back with `ok: false` and
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
) -> Result<OpResult> {
    network_op!(app, state, key, op_id, |h, progress, cancel| {
        let resolved = match target {
            Some(t) => Ok(t),
            None => match ops::implicit_push_target(&h.client, &branch).await {
                Ok(Some(t)) => Ok(t),
                Ok(None) => Err(GitError::InvalidPath { path: branch.clone(), reason: "the branch does not track a remote branch; choose where to push it".into() }),
                Err(e) => Err(e),
            },
        };
        match resolved {
            Ok(target) => ops::push(&h.client, &branch, &target, &progress, cancel).await,
            Err(e) => Err(e),
        }
    })
}

/// Delete `branch` (the name on the remote, `feature/x`) on `remote`, for everyone who uses that
/// remote: `git push <remote> --delete refs/heads/<branch>`, which also removes the
/// remote-tracking branch here. Cannot be undone from here; confirm with the user first. A remote
/// that no longer has the branch still returns `ok`, with "deleting a non-existent ref" in
/// stderr; one that refuses returns `ok: false` with `[remote rejected]` in stderr. When the
/// remote's branch has commits the remote-tracking branch here does not (someone pushed since the
/// last fetch), nothing is deleted and it returns `ok: false` with `(stale info)` in stderr.
/// `opId` works as for `opPull`; `opCancel` stops it at any point.
#[tauri::command]
#[specta::specta]
pub async fn op_delete_remote_branch(app: AppHandle, state: State<'_, AppState>, key: String, op_id: String, remote: String, branch: String) -> Result<OpResult> {
    network_op!(app, state, key, op_id, |h, progress, cancel| ops::delete_remote_branch(&h.client, &remote, &branch, &progress, cancel).await)
}

/// Make local `branch` track the remote branch whose remote-tracking branch is `upstream`, given
/// by its full name (`refs/remotes/origin/dev`), or with null stop it tracking any branch. Fails
/// with `invalidPath` when `upstream` is not under `refs/remotes/`. Git refuses an upstream that
/// does not exist, and unsetting when the branch tracks nothing.
#[tauri::command]
#[specta::specta]
pub async fn op_branch_set_upstream(app: AppHandle, state: State<'_, AppState>, key: String, branch: String, upstream: Option<String>) -> Result<OpResult> {
    write_op!(app, state, key, |c| ops::branch_set_upstream(c, &branch, upstream.as_deref()).await)
}

/// The remote branch local `branch` tracks (`branch.<branch>.remote` and `.merge`), or null.
#[tauri::command]
#[specta::specta]
pub async fn get_upstream(state: State<'_, AppState>, key: String, branch: String) -> Result<Option<PushTarget>> {
    let h = repo(&state, &key).await?;
    let _shared = h.lock.read().await;
    ops::upstream_of(&h.client, &branch).await
}

/// Where local `branch` goes when it is pushed without choosing a target: its upstream, or null
/// when it has none. Fails with `invalidPath` when the upstream has another name and `push.default`
/// does not push there (see `ops::push_target_for`); the Push dialog can still choose it.
#[tauri::command]
#[specta::specta]
pub async fn get_push_target(state: State<'_, AppState>, key: String, branch: String) -> Result<Option<PushTarget>> {
    let h = repo(&state, &key).await?;
    let _shared = h.lock.read().await;
    ops::implicit_push_target(&h.client, &branch).await
}

/// Set `push.default` to `mode` in this repository's own config, so a branch whose upstream has
/// another name is pushed to it (`upstream`) or to its own name (`current`) from now on, here and
/// by `git push` in a terminal.
#[tauri::command]
#[specta::specta]
pub async fn op_set_push_default(app: AppHandle, state: State<'_, AppState>, key: String, mode: ops::PushDefault) -> Result<OpResult> {
    write_op!(app, state, key, |c| ops::set_push_default(c, mode).await)
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

/// Rename local branch `oldName` to `newName`, keeping its upstream.
#[tauri::command]
#[specta::specta]
pub async fn op_branch_rename(app: AppHandle, state: State<'_, AppState>, key: String, old_name: String, new_name: String) -> Result<OpResult> {
    write_op!(app, state, key, |c| ops::branch_rename(c, &old_name, &new_name).await)
}

/// Move the current branch to `target`. `hard` also throws away every uncommitted change, so the
/// frontend confirms first.
#[tauri::command]
#[specta::specta]
pub async fn op_reset(app: AppHandle, state: State<'_, AppState>, key: String, mode: ResetMode, target: String) -> Result<OpResult> {
    write_op!(app, state, key, |c| ops::reset(c, mode, &target).await)
}

/// Apply `commit`'s change as a new commit on the current branch. Pass `mainline` (1 for the
/// branch merged into) for a merge commit, and null otherwise. When the result is not `ok`
/// and stderr mentions `--abort`, the cherry-pick is still open: `opCherryPickAbort` ends it.
#[tauri::command]
#[specta::specta]
pub async fn op_cherry_pick(app: AppHandle, state: State<'_, AppState>, key: String, commit: String, mainline: Option<u32>) -> Result<OpResult> {
    write_op!(app, state, key, |c| ops::cherry_pick(c, &commit, mainline).await)
}

#[tauri::command]
#[specta::specta]
pub async fn op_cherry_pick_abort(app: AppHandle, state: State<'_, AppState>, key: String) -> Result<OpResult> {
    write_op!(app, state, key, |c| ops::cherry_pick_abort(c).await)
}

/// Make a new commit that undoes `commit`. `mainline` and an unfinished revert work as for
/// `opCherryPick`; `opRevertAbort` ends one.
#[tauri::command]
#[specta::specta]
pub async fn op_revert(app: AppHandle, state: State<'_, AppState>, key: String, commit: String, mainline: Option<u32>) -> Result<OpResult> {
    write_op!(app, state, key, |c| ops::revert(c, &commit, mainline).await)
}

#[tauri::command]
#[specta::specta]
pub async fn op_revert_abort(app: AppHandle, state: State<'_, AppState>, key: String) -> Result<OpResult> {
    write_op!(app, state, key, |c| ops::revert_abort(c).await)
}

#[tauri::command]
#[specta::specta]
pub async fn op_tag_delete(app: AppHandle, state: State<'_, AppState>, key: String, name: String) -> Result<OpResult> {
    write_op!(app, state, key, |c| ops::tag_delete(c, &name).await)
}

/// The sync sequence against throwaway repositories, through a real `RepoHandle` opened the way
/// the app opens one (without an `AppHandle`, so nothing is emitted to a window).
#[cfg(test)]
mod e2e {
    use std::path::Path;
    use std::process::Command;
    use std::sync::Mutex;

    use super::*;
    use crate::platform::Host;
    use crate::settings::{Connection, RepoEntry, Settings};

    fn git(dir: &Path, args: &[&str]) {
        let st = Command::new("git").arg("-C").arg(dir).args(args).status().unwrap();
        assert!(st.success(), "git {args:?}");
    }

    fn commit(dir: &Path, file: &str, text: &str) {
        std::fs::write(dir.join(file), text).unwrap();
        git(dir, &["add", "-A"]);
        git(dir, &["commit", "-qm", file]);
    }

    fn rev(dir: &Path, r: &str) -> String {
        String::from_utf8(Command::new("git").arg("-C").arg(dir).args(["rev-parse", r]).output().unwrap().stdout).unwrap()
    }

    #[tokio::test]
    async fn sync_pulls_then_pushes_and_stops_where_it_should() {
        if which::which("git").is_err() { return; }
        let root = std::env::temp_dir().join(format!("stashpanda-sync-{}", uuid::Uuid::new_v4().simple()));
        let (origin, local, other) = (root.join("origin.git"), root.join("local"), root.join("other"));
        std::fs::create_dir_all(&root).unwrap();
        git(&root, &["init", "-q", "--bare", "-b", "main", origin.to_str().unwrap()]);
        for dir in [&local, &other] {
            git(&root, &["clone", "-q", origin.to_str().unwrap(), dir.to_str().unwrap()]);
            for (k, v) in [("user.email", "t@t"), ("user.name", "T"), ("commit.gpgsign", "false")] {
                git(dir, &["config", k, v]);
            }
        }
        commit(&local, "a.txt", "1\n");
        git(&local, &["push", "-q", "-u", "origin", "main"]);
        git(&other, &["pull", "-q", "origin", "main"]);

        let entry = RepoEntry {
            key: "sync-test".into(),
            path: local.to_string_lossy().into_owned(),
            connection: Connection::Local,
            resolved_transport: None,
            display_name: None,
            git_executable: None,
            group_id: None,
            last_opened_at: None,
            colour: None,
        };
        let h = crate::repo::open::open_repo(None, &entry, &Settings::default(), &Host::detect(), None).await.unwrap();
        let lines = Mutex::new(Vec::<(String, bool)>::new());
        let record = |line: String, cancellable: bool| lines.lock().unwrap().push((line, cancellable));

        // In sync already: nothing is pushed.
        let same = sync_steps(&h, "op-1", &record, "main", PullMode::Merge, false, None).await.unwrap();
        assert!(same.pull.ok && same.nothing_to_push && same.push.is_none(), "{same:?}");

        // Both sides have new commits: the pull merges, then the merge is pushed.
        commit(&other, "b.txt", "2\n");
        git(&other, &["push", "-q", "origin", "main"]);
        commit(&local, "c.txt", "3\n");
        let both = sync_steps(&h, "op-2", &record, "main", PullMode::Merge, false, None).await.unwrap();
        assert!(both.pull.ok && both.push.as_ref().is_some_and(|p| p.ok), "{both:?}");
        assert_eq!(rev(&origin, "main"), rev(&local, "HEAD"));
        assert_eq!(rev(&local, "HEAD^2"), rev(&other, "HEAD"));
        assert_eq!(both.target, PushTarget { remote: "origin".into(), branch: "main".into() });
        // The merge step is reported as not cancellable, the fetch and the push as cancellable.
        let seen = lines.lock().unwrap().clone();
        assert!(seen.iter().any(|(l, c)| l.starts_with("Merging origin/main") && !c), "{seen:?}");
        assert!(seen.iter().any(|(l, c)| l.starts_with("Pushing to origin/main") && *c), "{seen:?}");

        // Fast-forward only, with both sides moved on: the pull is refused and nothing is pushed.
        git(&other, &["pull", "-q", "origin", "main"]);
        commit(&other, "d.txt", "4\n");
        git(&other, &["push", "-q", "origin", "main"]);
        commit(&local, "e.txt", "5\n");
        let refused = sync_steps(&h, "op-3", &record, "main", PullMode::FastForwardOnly, false, None).await.unwrap();
        assert!(!refused.pull.ok && refused.push.is_none() && !refused.nothing_to_push, "{refused:?}");
        assert_ne!(rev(&origin, "main"), rev(&local, "HEAD"));

        // Stop during the merge is refused; Stop when the push starts leaves the merge in place
        // and reports the push as stopped.
        let refused_stop = Mutex::new(None);
        let stop_at_push = |line: String, cancellable: bool| {
            if line.starts_with("Merging") {
                *refused_stop.lock().unwrap() = Some(h.cancel_operation("op-4"));
            }
            if line.starts_with("Pushing to") {
                assert!(h.cancel_operation("op-4"));
            }
            let _ = cancellable;
        };
        let stopped = sync_steps(&h, "op-4", &stop_at_push, "main", PullMode::Merge, false, None).await.unwrap();
        assert_eq!(*refused_stop.lock().unwrap(), Some(false));
        assert!(stopped.pull.ok && stopped.push_stopped && stopped.push.is_none(), "{stopped:?}");
        assert_eq!(rev(&local, "HEAD^2"), rev(&other, "HEAD"));
        assert_ne!(rev(&origin, "main"), rev(&local, "HEAD"));

        // The remote's fetch refspec keeps no remote-tracking branch for dev, as in a
        // single-branch clone: the commits to push are counted from what was fetched.
        git(&local, &["config", "remote.origin.fetch", "+refs/heads/main:refs/remotes/origin/main"]);
        git(&local, &["checkout", "-q", "-b", "dev"]);
        git(&local, &["push", "-q", "-u", "origin", "dev"]);
        assert!(Command::new("git").arg("-C").arg(&local).args(["rev-parse", "--verify", "-q", "refs/remotes/origin/dev"]).status().unwrap().code() == Some(1));
        commit(&local, "f.txt", "6\n");
        let single = sync_steps(&h, "op-6", &record, "dev", PullMode::Merge, false, None).await.unwrap();
        assert!(single.pull.ok && single.push.as_ref().is_some_and(|p| p.ok), "{single:?}");
        assert_eq!(rev(&origin, "dev"), rev(&local, "HEAD"));

        // A branch that tracks a branch with another name: with push.default=simple, `git push`
        // refuses, so the sync refuses before it pulls. With push.default=current it pushes to
        // the branch's own name, never to the upstream.
        git(&local, &["checkout", "-q", "-b", "feature", "origin/main"]);
        git(&local, &["branch", "-q", "--set-upstream-to=origin/main", "feature"]);
        commit(&local, "g.txt", "7\n");
        let (main_before, head_before) = (rev(&origin, "main"), rev(&local, "HEAD"));
        git(&local, &["config", "push.default", "simple"]);
        assert!(matches!(sync_steps(&h, "op-7", &record, "feature", PullMode::Merge, false, None).await, Err(GitError::InvalidPath { .. })));
        assert_eq!((rev(&origin, "main"), rev(&local, "HEAD")), (main_before.clone(), head_before));
        git(&local, &["config", "push.default", "current"]);
        let current = sync_steps(&h, "op-8", &record, "feature", PullMode::Merge, false, None).await.unwrap();
        assert!(current.push.as_ref().is_some_and(|p| p.ok), "{current:?}");
        assert_eq!(current.push_target, PushTarget { remote: "origin".into(), branch: "feature".into() });
        assert_eq!(rev(&origin, "feature"), rev(&local, "HEAD"));
        assert_eq!(rev(&origin, "main"), main_before);

        // A target the user chose for this one sync is pushed to, though push.default refuses it.
        git(&local, &["config", "push.default", "simple"]);
        commit(&local, "h.txt", "8\n");
        let onto_main = PushTarget { remote: "origin".into(), branch: "main".into() };
        let chosen = sync_steps(&h, "op-9", &record, "feature", PullMode::Merge, false, Some(onto_main.clone())).await.unwrap();
        assert!(chosen.push.as_ref().is_some_and(|p| p.ok), "{chosen:?}");
        assert_eq!(chosen.push_target, onto_main);
        assert_eq!(rev(&origin, "main"), rev(&local, "HEAD"));

        // No upstream: nothing to sync with.
        git(&local, &["checkout", "-q", "-b", "loose"]);
        assert!(matches!(sync_steps(&h, "op-5", &record, "loose", PullMode::Merge, false, None).await, Err(GitError::InvalidPath { .. })));
        assert!(h.operations.lock().unwrap().is_empty(), "every operation unregistered");
        let _ = std::fs::remove_dir_all(&root);
    }
}
