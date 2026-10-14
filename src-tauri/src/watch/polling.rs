//! Fallback watcher: periodically hash `status` and `for-each-ref` output and
//! report a change when a hash differs. Used where the filesystem cannot
//! notify us and `inotifywait` is not available (see `remote.rs`), and on
//! filesystems the native watcher cannot watch.

use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{mpsc, watch};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use super::{Activity, GitDirChange, WatchEvent, WatcherHandle, WatcherKind};
use crate::git::queries::refs::FOR_EACH_REF_FORMAT;
use crate::git::{GitClient, GitReq};
use crate::settings::Polling;

pub struct PollingWatcher {
    cancel: CancellationToken,
}

impl WatcherHandle for PollingWatcher {
    fn kind(&self) -> WatcherKind {
        WatcherKind::Polling
    }
    fn stop(self: Box<Self>) {
        self.cancel.cancel();
    }
}

fn hash_bytes(b: &[u8]) -> u64 {
    let mut h = rustc_hash::FxHasher::default();
    b.hash(&mut h);
    h.finish()
}

/// How long polling waits between status checks at each activity level.
pub fn status_interval(settings: &Polling, activity: Activity) -> Duration {
    let ms = match activity {
        Activity::Focused => settings.status_interval_ms,
        Activity::Background => settings.status_background_interval_ms,
        Activity::Hidden => settings.status_background_interval_ms.saturating_mul(4),
    };
    Duration::from_millis(ms.max(1000))
}

/// Polls `status` at the interval the repository's activity sets, and the refs (where a fetch
/// elsewhere or a new tag shows) at `polling.refsIntervalMs` or the status interval, whichever is
/// longer; a commit or checkout made elsewhere already changes the status output. When the tab
/// becomes the focused one again, it checks at once rather than finishing a long wait.
pub fn start(
    client: Arc<GitClient>,
    settings: Polling,
    mut activity: watch::Receiver<Activity>,
    parent_cancel: &CancellationToken,
    tx: mpsc::UnboundedSender<WatchEvent>,
) -> PollingWatcher {
    let cancel = parent_cancel.child_token();
    let c2 = cancel.clone();
    tauri::async_runtime::spawn(async move {
        let mut last_status: Option<u64> = None;
        let mut last_refs: Option<u64> = None;
        let mut refs_checked: Option<Instant> = None;
        loop {
            let interval = status_interval(&settings, *activity.borrow_and_update());
            tokio::select! {
                _ = c2.cancelled() => break,
                _ = tokio::time::sleep(interval) => {}
                changed = activity.changed() => {
                    if changed.is_err() || *activity.borrow() != Activity::Focused {
                        continue;
                    }
                }
            }
            let status = client
                .exec_allow_failure(
                    GitReq::new("status", ["status", "--porcelain=v2", "-z", "--branch", "--untracked-files=all"]).watcher(),
                )
                .await;
            if let Ok(out) = status {
                let h = hash_bytes(&out.stdout);
                if last_status.is_some_and(|p| p != h) {
                    let _ = tx.send(WatchEvent::Worktree);
                }
                last_status = Some(h);
            }
            let refs_every = interval.max(Duration::from_millis(settings.refs_interval_ms));
            if refs_checked.is_some_and(|t| t.elapsed() < refs_every) {
                continue;
            }
            refs_checked = Some(Instant::now());
            let refs = client
                .exec_allow_failure(
                    GitReq::new(
                        "for-each-ref",
                        ["for-each-ref".to_string(), format!("--format={FOR_EACH_REF_FORMAT}"), "refs/heads".into(), "refs/remotes".into(), "refs/tags".into()],
                    )
                    .watcher(),
                )
                .await;
            if let Ok(out) = refs {
                let h = hash_bytes(&out.stdout);
                if last_refs.is_some_and(|p| p != h) {
                    let _ = tx.send(WatchEvent::GitDir(GitDirChange::Refs));
                }
                last_refs = Some(h);
            }
        }
    });
    PollingWatcher { cancel }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interval_follows_activity() {
        let p = Polling { status_interval_ms: 3000, status_background_interval_ms: 15000, refs_interval_ms: 10000, auto_fetch_minutes: None };
        assert_eq!(status_interval(&p, Activity::Focused), Duration::from_secs(3));
        assert_eq!(status_interval(&p, Activity::Background), Duration::from_secs(15));
        assert_eq!(status_interval(&p, Activity::Hidden), Duration::from_secs(60));
        let tiny = Polling { status_interval_ms: 10, ..p };
        assert_eq!(status_interval(&tiny, Activity::Focused), Duration::from_secs(1));
    }
}
