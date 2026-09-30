//! Turn a burst of watch events into one refresh: 300 ms of quiet, or at
//! most 2 s of continuous activity, then one `repo_changed` and the matching
//! graph refresh (status only, or refs and history). Nothing is emitted when
//! the refresh finds refs and status unchanged.

use std::sync::Arc;
use std::time::Duration;

use tauri::AppHandle;
use tauri_specta::Event;
use tokio::sync::mpsc;
use tokio::time::Instant;

use super::{RepoChangedEvent, WatchEvent, WatcherKind};
use crate::graph::loader::{refresh_if_changed, Changes};
use crate::repo::RepoHandle;
use crate::settings::{CacheSettings, GraphOptions};
use crate::storage::Db;

const QUIET: Duration = Duration::from_millis(300);
const MAX_HOLD: Duration = Duration::from_secs(2);

pub fn spawn(
    app: AppHandle,
    db: Arc<Db>,
    handle: Arc<RepoHandle>,
    kind: WatcherKind,
    options: GraphOptions,
    cache: CacheSettings,
    mut rx: mpsc::UnboundedReceiver<WatchEvent>,
) {
    let cancel = handle.cancel.child_token();
    tauri::async_runtime::spawn(async move {
        let mut status = false;
        let mut refs = false;
        let mut first: Option<Instant> = None;
        let mut deadline: Option<Instant> = None;
        loop {
            let sleep = async {
                match deadline {
                    Some(d) => tokio::time::sleep_until(d).await,
                    None => std::future::pending::<()>().await,
                }
            };
            tokio::select! {
                _ = cancel.cancelled() => break,
                ev = rx.recv() => {
                    let Some(ev) = ev else { break };
                    match ev {
                        WatchEvent::Worktree => status = true,
                        WatchEvent::GitDir(super::GitDirChange::Index) => status = true,
                        WatchEvent::GitDir(_) => { refs = true; status = true; }
                        WatchEvent::Overflow => { refs = true; status = true; }
                    }
                    let now = Instant::now();
                    let started = *first.get_or_insert(now);
                    deadline = Some((now + QUIET).min(started + MAX_HOLD));
                }
                _ = sleep => {
                    let (do_status, do_refs) = (status, refs);
                    status = false; refs = false; first = None; deadline = None;
                    if !(do_status || do_refs) { continue; }
                    // A write from the app has usually refreshed already, so
                    // this finds nothing new and emits nothing.
                    let changes = match refresh_if_changed(Some(&app), Some(&db), &handle, options, &cache, do_refs).await {
                        Ok(c) => c,
                        Err(e) => {
                            tracing::warn!("refresh after change failed for {}: {e}", handle.key);
                            Changes { status: do_status, refs: do_refs }
                        }
                    };
                    if !changes.any() { continue; }
                    let _ = RepoChangedEvent {
                        repo_id: handle.key.clone(),
                        status: changes.status,
                        refs: changes.refs,
                        cause: kind,
                    }
                    .emit(&app);
                }
            }
        }
    });
}
