//! Pick and start the watcher for an open repository, and its coalescer.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use tauri::AppHandle;
use tauri_specta::Event;
use tokio::sync::mpsc;

use super::RepoHandle;
use crate::settings::Settings;
use crate::storage::Db;
use crate::transport::paths::{HostPath, NativePath};
use crate::transport::TransportKind;
use crate::git::GitReq;
use crate::watch::native::IgnoredPaths;
use crate::watch::{coalesce, native, polling, WatchEvent, WatcherHandle, WatcherKind, WatcherStateEvent};

fn host_path_of(handle: &RepoHandle, native: &str) -> Option<HostPath> {
    if native.is_empty() {
        return None;
    }
    handle.transport.paths().to_host(&NativePath(native.to_string())).ok()
}

pub fn start(app: &AppHandle, db: &Arc<Db>, handle: &Arc<RepoHandle>, settings: &Settings) {
    let (tx, rx) = mpsc::unbounded_channel::<WatchEvent>();
    let mut detail: Option<String> = None;
    let ignored: IgnoredPaths = Arc::new(std::sync::RwLock::new(Vec::new()));

    let watcher: Option<Box<dyn WatcherHandle>> = match handle.transport.kind() {
        TransportKind::Local => {
            let worktree = if handle.info.is_bare { None } else { host_path_of(handle, &handle.info.toplevel) };
            let git_dir = host_path_of(handle, &handle.info.git_dir);
            let common = if handle.info.common_dir.is_empty() || handle.info.common_dir == "." {
                git_dir.clone()
            } else {
                // common_dir may be relative to git_dir
                let c = &handle.info.common_dir;
                if Path::new(c).is_absolute() || c.contains(':') { host_path_of(handle, c) } else {
                    git_dir.as_ref().map(|g| HostPath(format!("{}/{}", g.0.trim_end_matches(['/', '\\']), c)))
                }
            };
            match (git_dir, common) {
                (Some(gd), Some(cd)) => match native::start(
                    worktree.as_ref().map(|w| Path::new(&w.0)),
                    Path::new(&gd.0),
                    Path::new(&cd.0),
                    ignored.clone(),
                    tx.clone(),
                ) {
                    Ok(w) => Some(Box::new(w) as Box<dyn WatcherHandle>),
                    Err(e) => {
                        detail = Some(format!("native watcher unavailable ({e}); polling instead"));
                        None
                    }
                },
                _ => {
                    detail = Some("could not map the git directory to a local path; polling instead".into());
                    None
                }
            }
        }
        _ => {
            detail = Some("filesystem notifications are not available for this connection; polling instead".into());
            None
        }
    };

    let client = Arc::new(crate::git::GitClient {
        transport: handle.transport.clone(),
        repo: handle.native_path.clone(),
        caps: handle.client.caps,
        console: handle.client.console.clone(),
    });
    let (watcher, kind): (Box<dyn WatcherHandle>, WatcherKind) = match watcher {
        Some(w) => {
            spawn_ignore_refresher(handle.clone(), ignored.clone());
            (w, WatcherKind::Native)
        }
        None => {
            let interval = Duration::from_millis(settings.polling.status_interval_ms.max(1000));
            (Box::new(polling::start(client, interval, &handle.cancel, tx)), WatcherKind::Polling)
        }
    };
    if let Ok(mut slot) = handle.watcher.lock() {
        *slot = Some(watcher);
    }
    let _ = WatcherStateEvent { repo_id: handle.key.clone(), kind, detail }.emit(app);

    coalesce::spawn(
        app.clone(),
        db.clone(),
        handle.clone(),
        kind,
        settings.appearance.graph_order,
        settings.cache.clone(),
        rx,
    );
}

/// Keep the ignored-path list current: once at start, then every minute.
/// Without it the watcher reacts to build output, caches and logs that Git
/// itself never reports, and refreshes in a loop.
fn spawn_ignore_refresher(handle: Arc<RepoHandle>, ignored: IgnoredPaths) {
    let cancel = handle.cancel.child_token();
    tauri::async_runtime::spawn(async move {
        loop {
            let worktree_host = handle
                .transport
                .paths()
                .to_host(&NativePath(handle.info.toplevel.clone()))
                .ok();
            if let Some(wt) = worktree_host {
                let out = handle
                    .client
                    .exec_allow_failure(
                        GitReq::new("ls-files", ["ls-files", "-z", "--others", "--ignored", "--exclude-standard", "--directory"]).watcher(),
                    )
                    .await;
                if let Ok(out) = out {
                    if out.success() {
                        let base = Path::new(&wt.0);
                        let list: Vec<PathBuf> = crate::git::parse::split_nul(&out.stdout)
                            .into_iter()
                            .map(|rel| base.join(String::from_utf8_lossy(rel).trim_end_matches('/')))
                            .collect();
                        tracing::debug!("{} ignored paths for {}", list.len(), handle.key);
                        if let Ok(mut w) = ignored.write() {
                            *w = list;
                        }
                    }
                }
            }
            tokio::select! {
                _ = cancel.cancelled() => break,
                _ = tokio::time::sleep(Duration::from_secs(60)) => {}
            }
        }
    });
}
