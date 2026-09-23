//! Filesystem watching with `notify` (inotify on Linux, ReadDirectoryChangesW
//! on Windows) through the debouncer, for repositories on a local disk.

use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use notify_debouncer_full::notify::{EventKind, RecursiveMode};
use notify_debouncer_full::{new_debouncer, DebounceEventResult, Debouncer, RecommendedCache};
use notify_debouncer_full::notify::RecommendedWatcher;
use tokio::sync::mpsc;

use super::{classify_git_dir_path, WatchEvent, WatcherHandle, WatcherKind};
use crate::error::{GitError, Result};

pub struct NativeWatcher {
    debouncer: Debouncer<RecommendedWatcher, RecommendedCache>,
}

/// Absolute host paths Git ignores (from `ls-files --others --ignored
/// --directory`); events under them are dropped. Shared with the refresher.
pub type IgnoredPaths = Arc<RwLock<Vec<PathBuf>>>;

fn is_ignored(path: &Path, ignored: &IgnoredPaths) -> bool {
    match ignored.read() {
        Ok(list) => list.iter().any(|p| path.starts_with(p)),
        Err(_) => false,
    }
}

impl WatcherHandle for NativeWatcher {
    fn kind(&self) -> WatcherKind {
        WatcherKind::Native
    }
    fn stop(self: Box<Self>) {
        self.debouncer.stop();
    }
}

fn normalize(p: &Path) -> PathBuf {
    dunce::simplified(p).to_path_buf()
}

/// Map one changed path to a watch event, or None for noise.
pub fn map_path(path: &Path, worktree: Option<&Path>, git_dir: &Path, common_dir: &Path) -> Option<WatchEvent> {
    for dir in [git_dir, common_dir] {
        if let Ok(rel) = path.strip_prefix(dir) {
            let rel = rel.to_string_lossy();
            return classify_git_dir_path(&rel).map(WatchEvent::GitDir);
        }
    }
    if let Some(wt) = worktree {
        if path.strip_prefix(wt).is_ok() {
            if path.file_name().is_some_and(|n| n.to_string_lossy().ends_with(".lock")) {
                return None;
            }
            return Some(WatchEvent::Worktree);
        }
    }
    None
}

/// Start watching. `worktree` is None for a bare repository. Paths are host
/// paths (what this process can open).
pub fn start(
    worktree: Option<&Path>,
    git_dir: &Path,
    common_dir: &Path,
    ignored: IgnoredPaths,
    tx: mpsc::UnboundedSender<WatchEvent>,
) -> Result<NativeWatcher> {
    let worktree = worktree.map(normalize);
    let git_dir = normalize(git_dir);
    let common_dir = normalize(common_dir);
    let (wt2, gd2, cd2) = (worktree.clone(), git_dir.clone(), common_dir.clone());

    let mut debouncer = new_debouncer(
        Duration::from_millis(250),
        None,
        move |result: DebounceEventResult| match result {
            Ok(events) => {
                let mut sent_overflow = false;
                for ev in events {
                    // Reads are not changes. inotify reports every directory
                    // `git status` opens, which would make our own refresh
                    // trigger the next one forever.
                    if matches!(ev.kind, EventKind::Access(_)) {
                        continue;
                    }
                    if ev.need_rescan() && !sent_overflow {
                        sent_overflow = true;
                        let _ = tx.send(WatchEvent::Overflow);
                        continue;
                    }
                    for p in &ev.paths {
                        if is_ignored(p, &ignored) {
                            continue;
                        }
                        if let Some(w) = map_path(p, wt2.as_deref(), &gd2, &cd2) {
                            tracing::debug!("watch: {:?} {:?} <- {}", w, ev.kind, p.display());
                            let _ = tx.send(w);
                        }
                    }
                }
            }
            Err(errors) => {
                tracing::warn!("watcher errors: {errors:?}");
                let _ = tx.send(WatchEvent::Overflow);
            }
        },
    )
    .map_err(|e| GitError::Io { detail: format!("could not start file watcher: {e}") })?;

    let watch = |d: &mut Debouncer<RecommendedWatcher, RecommendedCache>, p: &Path, mode: RecursiveMode| -> Result<()> {
        d.watch(p, mode).map_err(|e| GitError::Io {
            detail: format!("could not watch {}: {e}", p.display()),
        })
    };

    match &worktree {
        // The worktree contains .git, so one recursive watch covers both.
        Some(wt) if git_dir.starts_with(wt) => watch(&mut debouncer, wt, RecursiveMode::Recursive)?,
        Some(wt) => {
            watch(&mut debouncer, wt, RecursiveMode::Recursive)?;
            watch(&mut debouncer, &git_dir, RecursiveMode::Recursive)?;
        }
        None => watch(&mut debouncer, &git_dir, RecursiveMode::Recursive)?,
    }
    if common_dir != git_dir && !common_dir.starts_with(worktree.as_deref().unwrap_or(&git_dir)) {
        // Linked worktree: refs live in the common dir.
        let _ = watch(&mut debouncer, &common_dir, RecursiveMode::NonRecursive);
        let _ = watch(&mut debouncer, &common_dir.join("refs"), RecursiveMode::Recursive);
    }
    Ok(NativeWatcher { debouncer })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_paths_to_events() {
        let wt = Path::new("/repo");
        let gd = Path::new("/repo/.git");
        assert_eq!(map_path(Path::new("/repo/src/a.rs"), Some(wt), gd, gd), Some(WatchEvent::Worktree));
        assert_eq!(map_path(Path::new("/repo/.git/objects/aa/bb"), Some(wt), gd, gd), None);
        assert_eq!(map_path(Path::new("/repo/.git/HEAD"), Some(wt), gd, gd), Some(WatchEvent::GitDir(super::super::GitDirChange::Head)));
        assert_eq!(map_path(Path::new("/repo/.git/index.lock"), Some(wt), gd, gd), None);
        assert_eq!(map_path(Path::new("/elsewhere/x"), Some(wt), gd, gd), None);
    }
}
