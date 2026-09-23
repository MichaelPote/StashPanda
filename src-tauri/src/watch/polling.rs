//! Fallback watcher: periodically hash `status` and `for-each-ref` output and
//! report a change when a hash differs. Used where the filesystem cannot
//! notify us (WSL paths seen from Windows, SSH, 9p mounts).

use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::{GitDirChange, WatchEvent, WatcherHandle, WatcherKind};
use crate::git::queries::refs::FOR_EACH_REF_FORMAT;
use crate::git::{GitClient, GitReq};

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

pub fn start(
    client: Arc<GitClient>,
    interval: Duration,
    parent_cancel: &CancellationToken,
    tx: mpsc::UnboundedSender<WatchEvent>,
) -> PollingWatcher {
    let cancel = parent_cancel.child_token();
    let c2 = cancel.clone();
    tauri::async_runtime::spawn(async move {
        let mut last_status: Option<u64> = None;
        let mut last_refs: Option<u64> = None;
        loop {
            tokio::select! {
                _ = c2.cancelled() => break,
                _ = tokio::time::sleep(interval) => {}
            }
            let status = client
                .exec_allow_failure(
                    GitReq::new("status", ["status", "--porcelain=v2", "-z", "--branch", "--untracked-files=all"]).watcher(),
                )
                .await;
            let refs = client
                .exec_allow_failure(
                    GitReq::new(
                        "for-each-ref",
                        ["for-each-ref".to_string(), format!("--format={FOR_EACH_REF_FORMAT}"), "refs/heads".into(), "refs/remotes".into(), "refs/tags".into()],
                    )
                    .watcher(),
                )
                .await;
            if let Ok(out) = status {
                let h = hash_bytes(&out.stdout);
                if last_status.is_some_and(|p| p != h) {
                    let _ = tx.send(WatchEvent::Worktree);
                }
                last_status = Some(h);
            }
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
