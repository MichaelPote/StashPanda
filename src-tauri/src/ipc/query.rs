//! Read-only queries against an open repository.

use std::sync::Arc;

use serde::Serialize;
use specta::Type;
use tauri::State;

use crate::error::{GitError, Result};
use crate::git::queries::blame::{get_blame, FileBlame};
use crate::git::queries::blob::{get_blob, get_worktree_file, BlobContent};
use crate::git::queries::history::{get_file_history as history_of, FileHistory};
use crate::git::queries::commit_files::{get_commit_files, FileChange};
use crate::git::queries::conflict_file::{get_conflict_file as conflict_file_of, ConflictFile};
use crate::git::queries::diff::{get_diff, join_repo_path, DiffOptions, DiffTarget, DiffText};
use crate::git::queries::log::{get_commit_body, get_stashes, get_unchanged_parents as unchanged_parents, StashEntry};
use crate::git::queries::refs::{get_refs, get_tag_annotation as tag_annotation_of, RefsSnapshot, TagAnnotation};
use crate::git::queries::full_status;
use crate::git::queries::status::StatusSnapshot;
use crate::repo::RepoHandle;
use crate::state::AppState;
use crate::transport::paths::NativePath;

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
    with_read_slot!(h, full_status(&h.client, &h.info.git_dir).await)
}

/// Last-modified time of each working-tree path (repository-relative), in milliseconds since
/// 1970, in the order given. Null for a path that no longer exists, such as a deleted file.
#[tauri::command]
#[specta::specta]
pub async fn get_modified_times(state: State<'_, AppState>, key: String, paths: Vec<String>) -> Result<Vec<Option<i64>>> {
    let h = repo(&state, &key).await?;
    let full: Vec<_> = paths.iter().map(|p| join_repo_path(&h.client.repo, p)).collect();
    with_read_slot!(h, h.client.transport.modified_times(&full).await)
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

/// Every Git setting that applies to the repository, from each file Git reads (system, global,
/// local, worktree), with the file it came from. Read through the repository's own transport, so
/// for a WSL or SSH repository the global settings are those of that machine.
#[tauri::command]
#[specta::specta]
pub async fn get_git_config(state: State<'_, AppState>, key: String) -> Result<Vec<crate::git::queries::config::ConfigEntry>> {
    let h = repo(&state, &key).await?;
    with_read_slot!(h, crate::git::queries::config::get_config_entries(&h.client).await)
}

/// Tag `name`'s message, tagger and date, for its pill's tooltip; `annotated: false` for a
/// lightweight tag, and null when there is no such tag.
#[tauri::command]
#[specta::specta]
pub async fn get_tag_annotation(state: State<'_, AppState>, key: String, name: String) -> Result<Option<TagAnnotation>> {
    let h = repo(&state, &key).await?;
    with_read_slot!(h, tag_annotation_of(&h.client, &name).await)
}

/// A merge commit's branch and parents, for the Files panel's merge summary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct MergeInfo {
    /// The branch the merge commit belongs to in the graph: the nearest branch tip above it on
    /// its lane, which is the branch it was merged into. None when no branch tip reaches it, as
    /// when that branch has been deleted.
    pub branch: Option<String>,
    /// In parent order.
    pub parents: Vec<MergeParent>,
}

/// One parent of a merge commit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct MergeParent {
    pub oid: String,
    /// The merge kept this parent's files as they were, so comparing with it lists no changes.
    pub unchanged: bool,
    /// The branch this parent belongs to in the graph: the nearest branch tip above it on its
    /// lane. None when no branch tip reaches it, or the commit is not in the graph.
    pub branch: Option<String>,
}

/// A merge commit's branch in the graph, and its parents in parent order.
#[tauri::command]
#[specta::specta]
pub async fn get_merge_info(state: State<'_, AppState>, key: String, commit: String) -> Result<MergeInfo> {
    let h = repo(&state, &key).await?;
    let parents = with_read_slot!(h, unchanged_parents(&h.client, &commit).await)?;
    let graph = h.graph.load_full();
    let branch_of = |oid: &str| graph.as_ref().and_then(|g| g.branch_of(oid)).map(String::from);
    Ok(MergeInfo {
        branch: branch_of(&commit),
        parents: parents
            .into_iter()
            .map(|(oid, unchanged)| MergeParent { branch: branch_of(&oid), oid, unchanged })
            .collect(),
    })
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
    with_read_slot!(h, get_blob(&h.client, Some((&h.catfile, &h.cancel)), &spec).await)
}

/// The commits that changed `path`, newest first, following renames, from `rev` (HEAD when
/// null), at most `maxCount` of them. A longer history is read by asking again with a larger
/// count: `git log --follow` cannot skip.
#[tauri::command]
#[specta::specta]
pub async fn get_file_history(
    state: State<'_, AppState>,
    key: String,
    path: String,
    rev: Option<String>,
    max_count: u32,
) -> Result<FileHistory> {
    let h = repo(&state, &key).await?;
    with_read_slot!(h, history_of(&h.client, &path, rev.as_deref(), max_count).await)
}

/// Who last changed each line of `path`: as it is in commit `rev`, or in the working tree when
/// `rev` is null.
#[tauri::command]
#[specta::specta]
pub async fn get_file_blame(state: State<'_, AppState>, key: String, path: String, rev: Option<String>) -> Result<FileBlame> {
    let h = repo(&state, &key).await?;
    with_read_slot!(h, get_blame(&h.client, Some((&h.catfile, &h.cancel)), &path, rev.as_deref()).await)
}

/// Conflicted file `path` as the merge editor opens it: its base, ours and theirs versions, and
/// the working tree file with its conflict markers replaced by their base text and described as
/// blocks. Every text uses "\n" line endings and has no byte order mark. A file the editor
/// cannot open (not conflicted, deleted on one side, binary, not UTF-8, over 5 MB) comes back
/// with `refused` set. Pass `hash` to `writeWorktreeFile` when saving.
#[tauri::command]
#[specta::specta]
pub async fn get_conflict_file(state: State<'_, AppState>, key: String, path: String) -> Result<ConflictFile> {
    let h = repo(&state, &key).await?;
    with_read_slot!(h, conflict_file_of(&h.client, Some((&h.catfile, &h.cancel)), &path).await)
}

#[tauri::command]
#[specta::specta]
pub async fn get_worktree_file_content(state: State<'_, AppState>, key: String, path: String) -> Result<BlobContent> {
    let h = repo(&state, &key).await?;
    with_read_slot!(h, get_worktree_file(&h.client, &path).await)
}

/// The message Git has prepared for the commit that finishes a merge, cherry-pick or revert
/// (`MERGE_MSG` in the git directory), without its comment lines; null when there is none.
#[tauri::command]
#[specta::specta]
pub async fn get_pending_message(state: State<'_, AppState>, key: String) -> Result<Option<String>> {
    let h = repo(&state, &key).await?;
    let path = NativePath(format!("{}/MERGE_MSG", h.info.git_dir.trim_end_matches(['/', '\\'])));
    let bytes = match h.transport.read_file(&path, 1024 * 1024).await {
        Ok(b) => b,
        Err(_) => return Ok(None),
    };
    let text = String::from_utf8_lossy(&bytes);
    let message: Vec<&str> = text.lines().filter(|l| !l.starts_with('#')).collect();
    let message = message.join("\n").trim().to_string();
    Ok((!message.is_empty()).then_some(message))
}

/// Whether the user is looking at the repository: the active tab in the focused window, the
/// active tab in a window in the background, or a tab that is not shown. Polling checks a
/// repository less often the less it is looked at; watchers that are told about changes ignore it.
#[tauri::command]
#[specta::specta]
pub async fn set_repo_activity(state: State<'_, AppState>, key: String, activity: crate::watch::Activity) -> Result<()> {
    let h = repo(&state, &key).await?;
    h.activity.send_replace(activity);
    Ok(())
}

/// Where the app can open or reveal `path` (relative to the top of the working tree), as this
/// computer sees it: `\\wsl.localhost\…` for a WSL repository from Windows, `/mnt/c/…` for a
/// Windows folder from WSL. Null for an SSH repository, whose files are on another machine.
#[tauri::command]
#[specta::specta]
pub async fn get_host_path(state: State<'_, AppState>, key: String, path: String) -> Result<Option<String>> {
    let h = repo(&state, &key).await?;
    let native = join_repo_path(&crate::transport::paths::NativePath(h.info.toplevel.clone()), &path);
    Ok(h.transport.paths().to_host(&native).ok().map(|p| p.0))
}

#[tauri::command]
#[specta::specta]
pub async fn get_repo_stashes(state: State<'_, AppState>, key: String) -> Result<Vec<StashEntry>> {
    let h = repo(&state, &key).await?;
    with_read_slot!(h, get_stashes(&h.client).await)
}
