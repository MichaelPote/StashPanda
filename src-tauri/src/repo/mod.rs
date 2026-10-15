//! One open repository: its transport, Git client, lock and lifecycle.

pub mod clone;
pub mod open;
pub mod resolve;
pub mod watch_start;

#[cfg(test)]
mod cross_e2e;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use arc_swap::ArcSwapOption;

use serde::{Deserialize, Serialize};
use specta::Type;
use tokio::sync::{Mutex, RwLock, Semaphore};
use tokio_util::sync::CancellationToken;

use crate::error::{GitError, Result};
use crate::git::queries::status::StatusSnapshot;
use crate::git::GitClient;
use crate::graph::snapshot::{GraphSnapshot, HiddenRefs};
use crate::settings::ResolvedTransport;
use crate::transport::paths::{HostPath, NativePath};
use crate::transport::SharedTransport;
use crate::watch::WatcherHandle;

/// What the frontend learns when a repository opens.
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct RepoInfo {
    pub key: String,
    pub display_name: String,
    /// Path as the app can show or open it (None for SSH).
    pub host_path: Option<String>,
    /// Path as `git -C` receives it.
    pub native_path: String,
    pub transport: ResolvedTransport,
    pub git_version: String,
    pub toplevel: String,
    pub git_dir: String,
    pub common_dir: String,
    pub is_bare: bool,
    pub is_linked_worktree: bool,
    pub warnings: Vec<String>,
}

pub struct RepoHandle {
    pub key: String,
    pub info: RepoInfo,
    pub transport: SharedTransport,
    pub client: GitClient,
    pub host_path: HostPath,
    pub native_path: NativePath,
    /// Reads take shared, short writes (stage, commit, checkout) exclusive.
    pub lock: RwLock<()>,
    /// Held by every write for its whole run, so writes never overlap. A pull holds only this
    /// one: holding `lock` for a network transfer would stall every read until it finished.
    pub write_gate: Mutex<()>,
    /// The running long operations (fetch, pull, push, sync), by the id the frontend gave.
    pub operations: OperationRegistry,
    /// Bounds concurrent read spawns for one repository.
    pub read_slots: Semaphore,
    pub cancel: CancellationToken,
    /// Latest graph snapshot, swapped whole on every refresh.
    pub graph: ArcSwapOption<GraphSnapshot>,
    pub graph_version: AtomicU64,
    pub last_status: RwLock<Option<StatusSnapshot>>,
    pub watcher: std::sync::Mutex<Option<Box<dyn WatcherHandle>>>,
    /// The watcher state last announced, for a frontend that reloaded to hear again.
    pub watcher_state: std::sync::Mutex<Option<crate::watch::WatcherStateEvent>>,
    /// The long-lived `cat-file` that blob reads go through.
    pub catfile: Arc<crate::git::catfile::CatFile>,
    /// Whether the user is looking at this repository; polling slows down when not.
    pub activity: tokio::sync::watch::Sender<crate::watch::Activity>,
    /// Refs switched off in the graph; read by every snapshot build.
    pub hidden: std::sync::RwLock<Arc<HiddenRefs>>,
    /// Colours the user chose for refs; read by every snapshot build.
    pub colours: std::sync::RwLock<Arc<crate::graph::snapshot::RefColours>>,
    /// The repository's own colour (a palette index), or `FIRST_COLOUR` without one: the first
    /// automatic lane colour in every snapshot build.
    pub first_colour: std::sync::atomic::AtomicU8,
}

impl RepoHandle {
    pub fn hidden_refs(&self) -> Arc<HiddenRefs> {
        self.hidden.read().expect("hidden refs lock").clone()
    }

    pub fn ref_colours(&self) -> Arc<crate::graph::snapshot::RefColours> {
        self.colours.read().expect("ref colours lock").clone()
    }

    /// `options` with this repository's first lane colour.
    pub fn graph_options(&self, options: crate::settings::GraphOptions) -> crate::settings::GraphOptions {
        crate::settings::GraphOptions { first_colour: self.first_colour.load(Ordering::Relaxed), ..options }
    }

    /// Sets the repository's own colour, or None for cyan; the next snapshot build uses it.
    pub fn set_first_colour(&self, colour: Option<u8>) {
        let c = colour.filter(|c| *c < crate::graph::lanes::HUE_COUNT).unwrap_or(crate::graph::lanes::FIRST_COLOUR);
        self.first_colour.store(c, Ordering::Relaxed);
    }

    /// Register a cancellable operation under `op_id` until the returned guard drops.
    pub fn start_operation(&self, op_id: &str) -> Result<OperationGuard<'_>> {
        start_operation(&self.operations, op_id, self.cancel.child_token())
    }

    /// Cancel a running operation. False when no operation has that id (it has already finished)
    /// or it is in a step that cannot be stopped.
    pub fn cancel_operation(&self, op_id: &str) -> bool {
        cancel_operation(&self.operations, op_id)
    }

    pub async fn close(self: Arc<Self>) {
        self.cancel.cancel();
        let w = self.watcher.lock().ok().and_then(|mut g| g.take());
        if let Some(w) = w {
            // Stopping joins the debouncer thread; keep it off the async runtime.
            let _ = tokio::task::spawn_blocking(move || w.stop()).await;
        }
        self.catfile.shutdown(&self.client).await;
        self.transport.shutdown().await;
    }
}

/// Running long operations by the id the frontend gave: a repository's, or the clones in
/// `AppState`, which have no repository yet.
pub type OperationRegistry = std::sync::Mutex<HashMap<String, RunningOperation>>;

/// Register a cancellable operation under `op_id`, stopped through `token`, until the returned
/// guard drops.
pub fn start_operation<'a>(registry: &'a OperationRegistry, op_id: &str, token: CancellationToken) -> Result<OperationGuard<'a>> {
    let mut ops = registry.lock().expect("operations lock");
    if op_id.is_empty() || ops.contains_key(op_id) {
        return Err(GitError::internal(format!("operation id {op_id:?} is empty or already in use")));
    }
    let cancellable = Arc::new(AtomicBool::new(true));
    ops.insert(op_id.to_string(), RunningOperation { token: token.clone(), cancellable: cancellable.clone() });
    Ok(OperationGuard { registry, op_id: op_id.to_string(), token, cancellable })
}

/// Cancel the operation registered under `op_id`. False when there is none (it has already
/// finished) or it is in a step that cannot be stopped.
pub fn cancel_operation(registry: &OperationRegistry, op_id: &str) -> bool {
    match registry.lock().expect("operations lock").get(op_id) {
        Some(op) if op.cancellable.load(Ordering::SeqCst) => {
            op.token.cancel();
            true
        }
        _ => false,
    }
}

/// A running operation's registration; dropping it removes the operation from the registry.
pub struct OperationGuard<'a> {
    registry: &'a OperationRegistry,
    op_id: String,
    pub token: CancellationToken,
    cancellable: Arc<AtomicBool>,
}

impl OperationGuard<'_> {
    /// Whether `cancel_operation` may stop the operation from now on. Set false around a step
    /// that must run to the end (a pull's merge or rebase); a cancel that arrived before it still
    /// shows in `token`.
    pub fn set_cancellable(&self, cancellable: bool) {
        self.cancellable.store(cancellable, Ordering::SeqCst);
    }

    /// Whether `cancel_operation` may stop the operation now.
    pub fn is_cancellable(&self) -> bool {
        self.cancellable.load(Ordering::SeqCst)
    }
}

/// A registry entry: the token that cancels the operation, and whether it may be cancelled now.
pub struct RunningOperation {
    token: CancellationToken,
    cancellable: Arc<AtomicBool>,
}

impl Drop for OperationGuard<'_> {
    fn drop(&mut self) {
        self.registry.lock().expect("operations lock").remove(&self.op_id);
    }
}
