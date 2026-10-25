//! Change detection for one repository, and the coalescer that turns bursts
//! of filesystem events into one refresh.

pub mod coalesce;
pub mod native;
pub mod polling;
pub mod remote;

use serde::{Deserialize, Serialize};
use specta::Type;
use tauri_specta::Event;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum WatcherKind {
    Native,
    RemoteInotify,
    Polling,
    Off,
}

/// How much the user is looking at a repository, which sets how often polling checks it. Watchers
/// that are told about changes (native, `inotifywait`) do not need it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum Activity {
    /// The active tab in the focused window: `polling.statusIntervalMs`.
    Focused,
    /// The active tab while the window is in the background: `polling.statusBackgroundIntervalMs`.
    Background,
    /// A tab that is not shown, or a minimised window: four times the background interval.
    Hidden,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GitDirChange {
    Head,
    Refs,
    PackedRefs,
    Index,
    MergeState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchEvent {
    Worktree,
    GitDir(GitDirChange),
    /// The backend lost events; refresh everything.
    Overflow,
}

/// Something the coalescer can stop when the repository closes.
pub trait WatcherHandle: Send {
    fn kind(&self) -> WatcherKind;
    fn stop(self: Box<Self>);
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
#[serde(rename_all = "camelCase")]
pub struct RepoChangedEvent {
    pub repo_id: String,
    pub status: bool,
    pub refs: bool,
    pub cause: WatcherKind,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
#[serde(rename_all = "camelCase")]
pub struct WatcherStateEvent {
    pub repo_id: String,
    pub kind: WatcherKind,
    pub detail: Option<String>,
}

/// Classify a path inside the git directory. None = ignore (objects, logs,
/// lock files and other noise).
pub fn classify_git_dir_path(rel: &str) -> Option<GitDirChange> {
    let rel = rel.trim_start_matches(['/', '\\']).replace('\\', "/");
    if rel.ends_with(".lock") {
        return None;
    }
    let first = rel.split('/').next().unwrap_or("");
    match first {
        "objects" | "lfs" | "logs" | "hooks" | "info" | "modules" | "worktrees" | "index.lock" => return None,
        // Upstreams and remotes live in config, and the Branches panel shows both.
        "refs" | "config" => return Some(GitDirChange::Refs),
        "packed-refs" => return Some(GitDirChange::PackedRefs),
        "index" => return Some(GitDirChange::Index),
        "HEAD" | "ORIG_HEAD" | "FETCH_HEAD" => return Some(GitDirChange::Head),
        "MERGE_HEAD" | "MERGE_MSG" | "CHERRY_PICK_HEAD" | "REVERT_HEAD" | "REBASE_HEAD" | "rebase-merge"
        | "rebase-apply" | "BISECT_LOG" | "AUTO_MERGE" | "SQUASH_MSG" => return Some(GitDirChange::MergeState),
        _ => {}
    }
    // Anything else at the top level of .git (description, COMMIT_EDITMSG...) is noise.
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_git_dir_paths() {
        assert_eq!(classify_git_dir_path("HEAD"), Some(GitDirChange::Head));
        assert_eq!(classify_git_dir_path("refs/heads/main"), Some(GitDirChange::Refs));
        assert_eq!(classify_git_dir_path("refs/heads/main.lock"), None);
        assert_eq!(classify_git_dir_path("objects/ab/cdef"), None);
        assert_eq!(classify_git_dir_path("index"), Some(GitDirChange::Index));
        assert_eq!(classify_git_dir_path("index.lock"), None);
        assert_eq!(classify_git_dir_path("rebase-merge/done"), Some(GitDirChange::MergeState));
        assert_eq!(classify_git_dir_path("COMMIT_EDITMSG"), None);
        assert_eq!(classify_git_dir_path("config"), Some(GitDirChange::Refs));
        assert_eq!(classify_git_dir_path("logs/HEAD"), None);
        assert_eq!(classify_git_dir_path("packed-refs"), Some(GitDirChange::PackedRefs));
    }
}
