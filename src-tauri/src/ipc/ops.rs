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
    ($app:expr, $state:expr, $key:expr, |$client:ident| $body:expr) => {
        write_op!(@run $app, $state, $key, false, |$client| $body)
    };
    // A write that deletes or renames refs, and so locks `packed-refs` as a fetch's `--prune` can:
    // it waits for a running fetch too.
    (refs $app:expr, $state:expr, $key:expr, |$client:ident| $body:expr) => {
        write_op!(@run $app, $state, $key, true, |$client| $body)
    };
    (@run $app:expr, $state:expr, $key:expr, $refs:expr, |$client:ident| $body:expr) => {{
        let h = repo(&$state, &$key).await?;
        let out = {
            let _network = if $refs { Some(h.network_gate.lock().await) } else { None };
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
/// kept; fetch it with `getCommitMessage(key, "HEAD")` to edit it instead. The checked-out
/// branch must still be `branch`, the one the dialog named (null: a detached HEAD); otherwise it
/// fails with `headMoved` and commits nothing.
#[tauri::command]
#[specta::specta]
#[allow(clippy::too_many_arguments)]
pub async fn op_commit(
    app: AppHandle,
    state: State<'_, AppState>,
    key: String,
    message: String,
    paths: Option<Vec<String>>,
    amend: bool,
    sign_off: bool,
    branch: Option<String>,
) -> Result<OpResult> {
    write_op!(app, state, key, |c| match ops::require_head(c, branch.as_deref()).await {
        Ok(()) => ops::commit(c, &message, paths.as_deref(), amend, sign_off).await,
        Err(e) => Err(e),
    })
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
    write_op!(refs app, state, key, |c| ops::branch_delete(c, &name, force).await)
}

/// Merge `target` into the current branch, which must still be `into` (null: a detached HEAD);
/// otherwise it fails with `headMoved` and merges nothing. `conflicts` in the result means
/// the merge stopped and the working tree has unmerged paths. `autostash` stashes uncommitted
/// changes first and puts them back once the merge is over: at once when it completes, or when a
/// stopped merge is continued or aborted.
#[tauri::command]
#[specta::specta]
#[allow(clippy::too_many_arguments)]
pub async fn op_merge(app: AppHandle, state: State<'_, AppState>, key: String, target: String, no_ff: bool, into: Option<String>, autostash: bool) -> Result<OpResult> {
    write_op!(app, state, key, |c| match ops::require_head(c, into.as_deref()).await {
        Ok(()) => ops::merge(c, &target, no_ff, autostash).await,
        Err(e) => Err(e),
    })
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

/// Rebase the current branch onto `onto` (a branch name or commit), not interactively. The
/// current branch must still be `branch` (null: a detached HEAD); otherwise it fails with
/// `headMoved` and rebases nothing. `autostash` stashes uncommitted changes first and puts them
/// back once the rebase is over: when it finishes, or when a stopped rebase is aborted.
/// `conflicts` in the result means the rebase stopped on a commit and is still open: the status's
/// `inProgress` then describes it, and `opContinue`, `opSkip` or `opRebaseAbort` go on.
#[tauri::command]
#[specta::specta]
pub async fn op_rebase(app: AppHandle, state: State<'_, AppState>, key: String, onto: String, branch: Option<String>, autostash: bool) -> Result<OpResult> {
    write_op!(app, state, key, |c| match ops::require_head(c, branch.as_deref()).await {
        Ok(()) => ops::rebase(c, &onto, autostash).await,
        Err(e) => Err(e),
    })
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

/// Pull `branch` from `remote` into local branch `into`, which must be the checked-out branch, as
/// `git fetch` and then `git merge` or `git rebase` (see `ops::pull_fetch` and
/// `ops::pull_integrate`). Fails with `headMoved`, merging nothing, when `into` is no longer
/// checked out by the time the pull runs or by the time the fetch is done. `opId` is chosen by the
/// caller: each progress update arrives as an `operationProgress` event carrying it, and
/// `opCancel` with it stops the pull while it is fetching, which then fails with `cancelled` and
/// changes no branch. Once the fetch is done, an event with `cancellable: false` says the local
/// step has begun, which always runs to the end. `autostash` stashes uncommitted changes first
/// and reapplies them afterwards.
///
/// Holds the network and write gates throughout, so no other fetch, push or write overlaps, but takes the exclusive lock only
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
    into: String,
    mode: PullMode,
    autostash: bool,
) -> Result<OpResult> {
    let h = repo(&state, &key).await?;
    let emit = progress_emitter(&app, &key, &op_id);
    let out = {
        let op = h.start_operation(&op_id)?;
        let _gate = gate_or_cancel(&h, &op.token).await?;
        pull_steps(&h, &op, &emit, &remote, &branch, &into, mode, autostash).await
    };
    after_write(&app, &state, &key).await;
    out.and_then(|r| match ops::index_lock_in(&r) {
        Some(lock_path) => Err(GitError::IndexLocked { lock_path }),
        None => Ok(r),
    })
}

/// A pull's two steps, for a caller holding the write gate: the fetch, which `op` can stop, then
/// the merge or rebase into `into` under the exclusive lock, during which it cannot. `into` must be
/// checked out before the fetch and still be before the merge. Leaves `op` uncancellable.
#[allow(clippy::too_many_arguments)]
async fn pull_steps(
    h: &RepoHandle,
    op: &OperationGuard<'_>,
    emit: &(dyn Fn(String, bool) + Send + Sync),
    remote: &str,
    branch: &str,
    into: &str,
    mode: PullMode,
    autostash: bool,
) -> Result<OpResult> {
    ops::require_checked_out(&h.client, into).await?;
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
            ops::require_checked_out(&h.client, into).await?;
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
/// when the branch has no upstream, and, before anything runs, when `git push` would refuse to
/// push it (see `ops::implicit_push_target`). The push goes where `git push` would send it, which
/// `remote.pushDefault`, `branch.<name>.pushRemote` or the remote's push refspecs can make another
/// remote or name than the upstream that was pulled. A `pushTo` the user chose
/// for this sync replaces where `git push` would push. Fails with `headMoved`, changing nothing,
/// when `branch` is not the checked-out branch.
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
        // Where `git push` would push, which `remote.pushDefault` or the remote's push refspecs
        // can make another remote or another name than the upstream.
        None => ops::implicit_push_target(&h.client, branch).await?.unwrap_or_else(|| target.clone()),
    };
    let pull = pull_steps(h, &op, emit, &target.remote, &target.branch, branch, mode, autostash).await?;
    if let Some(lock_path) = ops::index_lock_in(&pull) {
        return Err(GitError::IndexLocked { lock_path });
    }
    let mut result =
        SyncResult { target: target.clone(), push_target: push_target.clone(), pull, push: None, nothing_to_push: false, push_stopped: false, rewrote_pushed: false };
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
        Ok(r) => {
            // Only says why; a failure to tell leaves the usual "someone pushed" reading.
            result.rewrote_pushed = ops::push_rejected(&r) && ops::rewrote_pushed_commits(&h.client, branch, &push_target).await.unwrap_or(false);
            result.push = Some(r);
        }
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

/// Wait for `gate`. Cancelling while another operation still holds it stops the wait.
async fn lock_or_cancel<'a>(gate: &'a tokio::sync::Mutex<()>, token: &CancellationToken) -> Result<tokio::sync::MutexGuard<'a, ()>> {
    tokio::select! {
        biased;
        _ = token.cancelled() => Err(GitError::Cancelled),
        g = gate.lock() => Ok(g),
    }
}

/// Wait for the network gate and then the write gate, as a pull, push or sync does: no other
/// network operation and no other write overlaps it.
async fn gate_or_cancel<'a>(h: &'a RepoHandle, token: &CancellationToken) -> Result<(tokio::sync::MutexGuard<'a, ()>, tokio::sync::MutexGuard<'a, ()>)> {
    let network = lock_or_cancel(&h.network_gate, token).await?;
    let write = lock_or_cancel(&h.write_gate, token).await?;
    Ok((network, write))
}

/// Run a network operation that is cancellable from start to end (push, deleting a remote
/// branch): registered under `op_id`, under the network and write gates but not the exclusive lock, with progress as
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

/// Fetch from every remote (`git fetch --all --prune`), removing the remote-tracking branches of
/// branches the remotes no longer have. `opId` works as for `opPull`; `opCancel` stops the fetch
/// at any point, leaving the remote-tracking branches where they were.
#[tauri::command]
#[specta::specta]
pub async fn op_fetch(app: AppHandle, state: State<'_, AppState>, key: String, op_id: String) -> Result<OpResult> {
    // Only the network gate: a fetch changes remote-tracking branches, `FETCH_HEAD` and the object
    // store, none of which staging, committing or checking out writes, so those go ahead meanwhile.
    let h = repo(&state, &key).await?;
    let emit = progress_emitter(&app, &key, &op_id);
    let out = {
        let op = h.start_operation(&op_id)?;
        let _network = lock_or_cancel(&h.network_gate, &op.token).await?;
        let progress = |line: String| emit(line, true);
        ops::fetch_all(&h.client, &progress, &op.token).await
    };
    after_write(&app, &state, &key).await;
    out
}

/// Push local `branch`. With no `target`, where plain `git push` would push it (failing with
/// `invalidPath` when it has no upstream, or when Git would refuse: call `getPushTarget` first);
/// with a `target` the user chose, there. It never sets the
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

/// Push tags to `remote`: every tag, the named ones, or those on a commit's history (what a push
/// of a branch at that commit carries). Never forces: a tag the remote already has on another
/// commit comes back with `ok: false`, `[rejected]` and "(already exists)" in stderr, and the
/// other tags are still pushed. With no tag to push, `ok` with "No tags to push." as stdout.
/// `opId` works as for `opPush`; `opCancel` stops it at any point.
#[tauri::command]
#[specta::specta]
pub async fn op_push_tags(app: AppHandle, state: State<'_, AppState>, key: String, op_id: String, remote: String, tags: ops::TagsToPush) -> Result<OpResult> {
    network_op!(app, state, key, op_id, |h, progress, cancel| ops::push_tags(&h.client, &remote, &tags, &progress, cancel).await)
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

/// Where local `branch` goes when it is pushed without choosing a target, as `git push` decides it
/// from `branch.<name>.pushRemote`, `remote.pushDefault`, the remote's push refspecs, the upstream
/// and `push.default`; null when it has no upstream and nothing else says. Fails with
/// `invalidPath` when `git push` would refuse, or would push somewhere that is not a branch (see
/// `ops::implicit_push_target`); the Push dialog can still choose a target.
#[tauri::command]
#[specta::specta]
pub async fn get_push_target(state: State<'_, AppState>, key: String, branch: String) -> Result<Option<PushTarget>> {
    let h = repo(&state, &key).await?;
    let _shared = h.lock.read().await;
    ops::implicit_push_target(&h.client, &branch).await
}

/// True when HEAD is already on the remote branch that local `branch` is pushed to (as last
/// fetched), so amending it and pushing would need a force push. False when `branch` has nowhere
/// `git push` would send it.
#[tauri::command]
#[specta::specta]
pub async fn get_head_on_push_target(state: State<'_, AppState>, key: String, branch: String) -> Result<bool> {
    let h = repo(&state, &key).await?;
    let _shared = h.lock.read().await;
    match ops::implicit_push_target(&h.client, &branch).await {
        Ok(Some(target)) => ops::head_on_push_target(&h.client, &target).await,
        Ok(None) | Err(GitError::InvalidPath { .. } | GitError::PushTargetRefused { .. }) => Ok(false),
        Err(e) => Err(e),
    }
}

/// After a push of local `branch` to `target` was refused, true when the branch lacks the
/// target's commits because they were rewritten here (an amend or a rebase), not because someone
/// else pushed: pulling cannot fix that, and only a force push would replace them.
#[tauri::command]
#[specta::specta]
pub async fn get_rewrote_pushed_commits(state: State<'_, AppState>, key: String, branch: String, target: PushTarget) -> Result<bool> {
    let h = repo(&state, &key).await?;
    let _shared = h.lock.read().await;
    ops::rewrote_pushed_commits(&h.client, &branch, &target).await
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

/// True when local branch `branch` is not checked out, is behind HEAD, and lies on the checked-out
/// branch's own first-parent line, so `opBranchFastForward` can move it up to HEAD. The branch's
/// menu offers "Fast-forward to <current branch>" only then.
#[tauri::command]
#[specta::specta]
pub async fn get_fast_forwards_to_head(state: State<'_, AppState>, key: String, branch: String) -> Result<bool> {
    let h = repo(&state, &key).await?;
    let _shared = h.lock.read().await;
    ops::fast_forwards_to_head(&h.client, &branch).await
}

/// Move local branch `branch` forward to the checked-out branch `current`, without checking it
/// out, so uncommitted changes are fine: `git fetch . <current>:<branch>`. Refused with `ok:
/// false`, changing nothing, when it would not be a fast-forward (`[rejected]` in stderr) or when
/// a worktree has the branch checked out.
#[tauri::command]
#[specta::specta]
pub async fn op_branch_fast_forward(app: AppHandle, state: State<'_, AppState>, key: String, branch: String, current: String) -> Result<OpResult> {
    write_op!(app, state, key, |c| ops::push_to_local_branch(c, &current, &branch).await)
}

/// The commits a force push of commit `source` over a branch now at commit `tip` would drop from
/// it, newest first (at most 50), and how many in all: what the Push To dialog lists before a
/// force push, for the same `source` and `tip` it then passes to `opPushTo`.
#[tauri::command]
#[specta::specta]
pub async fn get_commits_dropped(state: State<'_, AppState>, key: String, source: String, tip: String) -> Result<ops::DroppedCommits> {
    let h = repo(&state, &key).await?;
    let _shared = h.lock.read().await;
    ops::commits_dropped(&h.client, &source, &tip, 50).await
}

/// Fast-forward local branch `branch` to its upstream as last fetched, without fetching: the
/// checked-out branch with `git merge --ff-only`, stashing uncommitted changes first and putting
/// them back afterwards when `autostash` (a conflict putting them back leaves `ok` with Git's
/// autostash conflict message in the output, and the changes kept as `stash@{0}`); any other
/// branch with `git fetch . <upstream>:<branch>`, which touches no files. Not a fast-forward:
/// `ok: false`. A branch another worktree, a rebase or a bisect is using: `ok: false` with
/// "refusing to move" in stderr. No upstream, or one never fetched: `invalidPath`.
#[tauri::command]
#[specta::specta]
pub async fn op_branch_fast_forward_to_upstream(app: AppHandle, state: State<'_, AppState>, key: String, branch: String, autostash: bool) -> Result<OpResult> {
    write_op!(app, state, key, |c| ops::fast_forward_to_upstream(c, &branch, autostash).await)
}

/// Push local branch `branch` to branch `target` on `remote`, or, with no `remote`, to local
/// branch `target` of this repository (`git fetch . <branch>:<target>`, which checks nothing out).
/// Without `force`, only a fast-forward is allowed: a refusal comes back with `ok: false` and
/// `[rejected]` in stderr. With `force`, both ends are pinned to the commits the dropped list was
/// made from: `source`, which `branch` must still point at (otherwise `ok: false` with "source
/// moved" in stderr), is what is sent, and the target must still be at `expected` (a remote push
/// fails with `(stale info)` in stderr; a local move is refused by `git update-ref`). Never sets an upstream. `opId` works as for `opPush`
/// for a remote; a local push is too quick to stop.
#[tauri::command]
#[specta::specta]
#[allow(clippy::too_many_arguments)]
pub async fn op_push_to(
    app: AppHandle,
    state: State<'_, AppState>,
    key: String,
    op_id: String,
    branch: String,
    remote: Option<String>,
    target: String,
    force: bool,
    expected: Option<String>,
    source: Option<String>,
) -> Result<OpResult> {
    let pinned = || -> Result<(String, String)> {
        match (&source, &expected) {
            (Some(s), Some(e)) => Ok((s.clone(), e.clone())),
            _ => Err(GitError::InvalidPath { path: target.clone(), reason: "a force push needs the commits its dropped list was made from".into() }),
        }
    };
    match remote {
        Some(remote) => network_op!(app, state, key, op_id, |h, progress, cancel| {
            let to = PushTarget { remote, branch: target.clone() };
            if force {
                match pinned() {
                    Ok((source, expected)) => ops::force_push(&h.client, &branch, &source, &to, &expected, &progress, cancel).await,
                    Err(e) => Err(e),
                }
            } else {
                ops::push(&h.client, &branch, &to, &progress, cancel).await
            }
        }),
        None if force => {
            let (source, expected) = pinned()?;
            write_op!(app, state, key, |c| ops::force_move_local_branch(c, &branch, &source, &target, &expected).await)
        }
        None => write_op!(app, state, key, |c| ops::push_to_local_branch(c, &branch, &target).await),
    }
}

/// Add remote `name` at `url`, without fetching it (call `opFetch` for that). Git refuses a name
/// that is already a remote, with `ok: false`.
#[tauri::command]
#[specta::specta]
pub async fn op_remote_add(app: AppHandle, state: State<'_, AppState>, key: String, name: String, url: String) -> Result<OpResult> {
    write_op!(app, state, key, |c| ops::remote_add(c, &name, &url).await)
}

/// Set remote `name`'s URL, and its separate push URL, or with a null `pushUrl` remove any so
/// pushes go to `url` as well.
#[tauri::command]
#[specta::specta]
pub async fn op_remote_set_url(app: AppHandle, state: State<'_, AppState>, key: String, name: String, url: String, push_url: Option<String>) -> Result<OpResult> {
    write_op!(app, state, key, |c| ops::remote_set_url(c, &name, &url, push_url.as_deref()).await)
}

/// Rename remote `oldName` to `newName`, with its remote-tracking branches and the upstreams of
/// the branches that track it. Waits for a running fetch, since it rewrites refs.
#[tauri::command]
#[specta::specta]
pub async fn op_remote_rename(app: AppHandle, state: State<'_, AppState>, key: String, old_name: String, new_name: String) -> Result<OpResult> {
    write_op!(refs app, state, key, |c| ops::remote_rename(c, &old_name, &new_name).await)
}

/// Remove remote `name` from this repository: its remote-tracking branches are deleted and the
/// branches that tracked it track nothing. The remote itself is untouched. Confirm with the user
/// first. Waits for a running fetch, since it deletes refs.
#[tauri::command]
#[specta::specta]
pub async fn op_remote_remove(app: AppHandle, state: State<'_, AppState>, key: String, name: String) -> Result<OpResult> {
    write_op!(refs app, state, key, |c| ops::remote_remove(c, &name).await)
}

/// Rename local branch `oldName` to `newName`, keeping its upstream.
#[tauri::command]
#[specta::specta]
pub async fn op_branch_rename(app: AppHandle, state: State<'_, AppState>, key: String, old_name: String, new_name: String) -> Result<OpResult> {
    write_op!(refs app, state, key, |c| ops::branch_rename(c, &old_name, &new_name).await)
}

/// Move the current branch to `target`. `hard` also throws away every uncommitted change, so the
/// frontend confirms first.
/// The current branch must still be `branch` (null: a detached HEAD), the one the confirmation
/// named; otherwise it fails with `headMoved` and moves nothing.
#[tauri::command]
#[specta::specta]
pub async fn op_reset(app: AppHandle, state: State<'_, AppState>, key: String, mode: ResetMode, target: String, branch: Option<String>) -> Result<OpResult> {
    write_op!(app, state, key, |c| match ops::require_head(c, branch.as_deref()).await {
        Ok(()) => ops::reset(c, mode, &target).await,
        Err(e) => Err(e),
    })
}

/// Apply `commit`'s change as a new commit on the current branch. Pass `mainline` (1 for the
/// branch merged into) for a merge commit, and null otherwise. When the result is not `ok`
/// and stderr mentions `--abort`, the cherry-pick is still open: `opCherryPickAbort` ends it.
/// The checked-out branch must still be `branch` (null: a detached HEAD); otherwise it fails with
/// `headMoved` and changes nothing.
#[tauri::command]
#[specta::specta]
pub async fn op_cherry_pick(app: AppHandle, state: State<'_, AppState>, key: String, commit: String, mainline: Option<u32>, branch: Option<String>) -> Result<OpResult> {
    write_op!(app, state, key, |c| match ops::require_head(c, branch.as_deref()).await {
        Ok(()) => ops::cherry_pick(c, &commit, mainline).await,
        Err(e) => Err(e),
    })
}

#[tauri::command]
#[specta::specta]
pub async fn op_cherry_pick_abort(app: AppHandle, state: State<'_, AppState>, key: String) -> Result<OpResult> {
    write_op!(app, state, key, |c| ops::cherry_pick_abort(c).await)
}

/// Make a new commit that undoes `commit`. `mainline` and an unfinished revert work as for
/// `opCherryPick`; `opRevertAbort` ends one. `branch` is checked as for `opCherryPick`.
#[tauri::command]
#[specta::specta]
pub async fn op_revert(app: AppHandle, state: State<'_, AppState>, key: String, commit: String, mainline: Option<u32>, branch: Option<String>) -> Result<OpResult> {
    write_op!(app, state, key, |c| match ops::require_head(c, branch.as_deref()).await {
        Ok(()) => ops::revert(c, &commit, mainline).await,
        Err(e) => Err(e),
    })
}

#[tauri::command]
#[specta::specta]
pub async fn op_revert_abort(app: AppHandle, state: State<'_, AppState>, key: String) -> Result<OpResult> {
    write_op!(app, state, key, |c| ops::revert_abort(c).await)
}

#[tauri::command]
#[specta::specta]
pub async fn op_tag_delete(app: AppHandle, state: State<'_, AppState>, key: String, name: String) -> Result<OpResult> {
    write_op!(refs app, state, key, |c| ops::tag_delete(c, &name).await)
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
