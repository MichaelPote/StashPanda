//! One open repository: its transport, Git client, lock and lifecycle.

pub mod open;
pub mod resolve;
pub mod watch_start;

use std::sync::atomic::AtomicU64;
use std::sync::Arc;

use arc_swap::ArcSwapOption;

use serde::{Deserialize, Serialize};
use specta::Type;
use tokio::sync::{RwLock, Semaphore};
use tokio_util::sync::CancellationToken;

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
    /// Reads take shared, writes (stage, commit, checkout, fetch) exclusive.
    pub lock: RwLock<()>,
    /// Bounds concurrent read spawns for one repository.
    pub read_slots: Semaphore,
    pub cancel: CancellationToken,
    /// Latest graph snapshot, swapped whole on every refresh.
    pub graph: ArcSwapOption<GraphSnapshot>,
    pub graph_version: AtomicU64,
    pub last_status: RwLock<Option<StatusSnapshot>>,
    pub watcher: std::sync::Mutex<Option<Box<dyn WatcherHandle>>>,
    /// Refs switched off in the graph; read by every snapshot build.
    pub hidden: std::sync::RwLock<Arc<HiddenRefs>>,
}

impl RepoHandle {
    pub fn hidden_refs(&self) -> Arc<HiddenRefs> {
        self.hidden.read().expect("hidden refs lock").clone()
    }

    pub async fn close(self: Arc<Self>) {
        self.cancel.cancel();
        let w = self.watcher.lock().ok().and_then(|mut g| g.take());
        if let Some(w) = w {
            // Stopping joins the debouncer thread; keep it off the async runtime.
            let _ = tokio::task::spawn_blocking(move || w.stop()).await;
        }
        self.transport.shutdown().await;
    }
}
