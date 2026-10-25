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
use crate::watch::{coalesce, native, polling, remote, WatchEvent, WatcherHandle, WatcherKind, WatcherStateEvent};

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
            // Absolute since `open_repo`; empty only for a repository opened before that.
            let common = if handle.info.common_dir.is_empty() { git_dir.clone() } else { host_path_of(handle, &handle.info.common_dir) };
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
        // Polls until `try_remote_inotify` has `inotifywait` running on the other system.
        _ => None,
    };

    let client = Arc::new(crate::git::GitClient {
        transport: handle.transport.clone(),
        repo: handle.native_path.clone(),
        caps: handle.client.caps,
        console: handle.client.console.clone(),
    });
    let remote = !matches!(handle.transport.kind(), TransportKind::Local);
    let (watcher, kind): (Box<dyn WatcherHandle>, WatcherKind) = match watcher {
        Some(w) => {
            spawn_ignore_refresher(handle.clone(), ignored.clone());
            (w, WatcherKind::Native)
        }
        None => {
            let poller = polling::start(client.clone(), settings.polling.clone(), handle.activity.subscribe(), &handle.cancel, tx.clone());
            (Box::new(poller), WatcherKind::Polling)
        }
    };
    if let Ok(mut slot) = handle.watcher.lock() {
        *slot = Some(watcher);
    }
    emit_state(app, handle, kind, detail);

    coalesce::spawn(
        app.clone(),
        db.clone(),
        handle.clone(),
        kind,
        settings.appearance.graph_options(),
        settings.cache.clone(),
        rx,
    );
    if remote {
        try_remote_inotify(app.clone(), handle.clone(), client, settings.polling.clone(), tx);
    }
}

/// How long to wait before asking a remote system about `inotifywait` again, while it cannot be
/// reached, and before starting it again after it stopped with the connection.
const RETRY_INOTIFY: Duration = Duration::from_secs(30);

/// Where the repository's files are, for messages: the SSH host or the WSL distribution.
fn remote_name(handle: &RepoHandle) -> String {
    match handle.transport.kind() {
        TransportKind::Ssh { host } => host,
        TransportKind::Wsl { distro } => format!("WSL ({distro})"),
        TransportKind::Local => "this computer".into(),
    }
}

/// Announces the watcher in use, and keeps it for `repo_open` to announce again to a frontend that
/// reloaded.
fn emit_state(app: &AppHandle, handle: &RepoHandle, kind: WatcherKind, detail: Option<String>) {
    let ev = WatcherStateEvent { repo_id: handle.key.clone(), kind, detail };
    if let Ok(mut last) = handle.watcher_state.lock() {
        *last = Some(ev.clone());
    }
    let _ = ev.emit(app);
}

/// Puts `next` in the repository's watcher slot and stops the one it replaces. False, and `next`
/// stopped, when the repository has closed meanwhile.
fn swap_watcher(handle: &RepoHandle, next: Box<dyn WatcherHandle>) -> bool {
    if handle.cancel.is_cancelled() {
        next.stop();
        return false;
    }
    let old = handle.watcher.lock().ok().and_then(|mut slot| slot.replace(next));
    if let Some(old) = old {
        old.stop();
    }
    true
}

/// The folders Git ignores in the working tree, as absolute paths on the repository's own system.
async fn ignored_dirs(handle: &RepoHandle) -> Vec<String> {
    if handle.info.is_bare {
        return Vec::new();
    }
    let req = GitReq::new("ls-files", ["ls-files", "-z", "--others", "--ignored", "--exclude-standard", "--directory"]);
    match handle.client.exec_allow_failure(req).await {
        Ok(out) if out.success() => crate::git::parse::split_nul(&out.stdout)
            .into_iter()
            .map(|rel| String::from_utf8_lossy(rel).into_owned())
            .filter(|rel| rel.ends_with('/'))
            .map(|rel| format!("{}/{}", handle.info.toplevel.trim_end_matches('/'), rel.trim_end_matches('/')))
            .collect(),
        _ => Vec::new(),
    }
}

/// For a repository reached over SSH or WSL: replaces polling with `inotifywait` on the system
/// that holds it, when that system has it. If `inotifywait` stops later (the connection dropped,
/// the limit on watched folders was reached), polling takes over again. Each switch is announced
/// with `watcher_state`, whose `detail` says why.
fn try_remote_inotify(app: AppHandle, handle: Arc<RepoHandle>, client: Arc<crate::git::GitClient>, polling_settings: crate::settings::Polling, tx: mpsc::UnboundedSender<WatchEvent>) {
    tauri::async_runtime::spawn(async move {
        let every = polling::status_interval(&polling_settings, crate::watch::Activity::Focused).as_secs();
        let name = remote_name(&handle);
        // While the connection is down the system cannot be asked; try again until it can.
        loop {
            match remote::available(&handle.transport).await {
                Ok(true) => break,
                Ok(false) => {
                    emit_state(&app, &handle, WatcherKind::Polling, Some(format!(
                        "inotifywait is not installed on {name}, so changes are found by checking every {every} seconds. Install inotify-tools there to see them at once.",
                    )));
                    return;
                }
                Err(_) => {
                    tokio::select! {
                        _ = handle.cancel.cancelled() => return,
                        _ = tokio::time::sleep(RETRY_INOTIFY) => {}
                    }
                }
            }
        }
        let ignored = ignored_dirs(&handle).await;
        let worktree = (!handle.info.is_bare).then(|| handle.info.toplevel.trim_end_matches('/').to_string());
        let git_dir = handle.info.git_dir.trim_end_matches('/').to_string();
        let common_dir = if handle.info.common_dir.is_empty() { git_dir.clone() } else { handle.info.common_dir.trim_end_matches('/').to_string() };
        let on_exit = {
            let (app, handle, tx) = (app.clone(), handle.clone(), tx.clone());
            move |stderr: String| {
                if handle.cancel.is_cancelled() {
                    return;
                }
                let poller = polling::start(client.clone(), polling_settings.clone(), handle.activity.subscribe(), &handle.cancel, tx.clone());
                if !swap_watcher(&handle, Box::new(poller)) {
                    return;
                }
                let why = if stderr.is_empty() { String::new() } else { format!(": {stderr}") };
                let final_exit = remote::exit_is_final(&stderr);
                let then = if final_exit { "" } else { " It starts again once the connection is back." };
                emit_state(&app, &handle, WatcherKind::Polling, Some(format!(
                    "inotifywait on {name} stopped{why}. Changes are found by checking every {every} seconds instead.{then}",
                )));
                if !final_exit {
                    // Usually the connection dropped: wait a little, then start it again; the
                    // loop above waits for the connection to come back.
                    tauri::async_runtime::spawn(async move {
                        tokio::select! {
                            _ = handle.cancel.cancelled() => {}
                            _ = tokio::time::sleep(RETRY_INOTIFY) => try_remote_inotify(app, handle, client, polling_settings, tx),
                        }
                    });
                }
            }
        };
        match remote::start(handle.transport.clone(), worktree, git_dir, common_dir, &ignored, &handle.cancel, tx, on_exit).await {
            Ok(w) => {
                if swap_watcher(&handle, Box::new(w)) {
                    emit_state(&app, &handle, WatcherKind::RemoteInotify, None);
                }
            }
            Err(e) => emit_state(&app, &handle, WatcherKind::Polling, Some(format!(
                "inotifywait could not start on {} ({e}), so changes are found by checking every {every} seconds.",
                remote_name(&handle),
            ))),
        }
    });
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
                        GitReq::new("ls-files", ["ls-files", "-z", "--others", "--ignored", "--exclude-standard", "--directory"]),
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
