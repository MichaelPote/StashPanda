//! Open a repository from its library entry: resolve, probe, rev-parse, register.

use std::sync::Arc;

use tauri::AppHandle;
use tokio::sync::{RwLock, Semaphore};
use tokio_util::sync::CancellationToken;

use super::resolve::resolve;
use super::{RepoHandle, RepoInfo};
use crate::error::{GitError, Result};
use crate::git::console::ConsoleRecorder;
use crate::git::versions::Capabilities;
use crate::git::{GitClient, GitReq};
use crate::platform::Host;
use crate::settings::{RepoEntry, Settings};
use crate::transport::paths::NativePath;
use crate::transport::ssh_pool::SshContext;

fn display_name_for(entry: &RepoEntry, native: &NativePath) -> String {
    if let Some(n) = &entry.display_name {
        if !n.trim().is_empty() {
            return n.clone();
        }
    }
    native
        .0
        .trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\'])
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or(&native.0)
        .to_string()
}

pub async fn open_repo(
    app: Option<&AppHandle>,
    entry: &RepoEntry,
    settings: &Settings,
    host: &Host,
    ssh: Option<&Arc<SshContext>>,
) -> Result<Arc<RepoHandle>> {
    let res = resolve(entry, settings, host, ssh)?;
    let console = Arc::new(ConsoleRecorder::new(
        app.cloned(),
        entry.key.clone(),
        settings.console.output_cap_bytes as usize,
    ));

    // Probe: git --version through the transport (logged like any command).
    let probe_client = GitClient {
        transport: res.transport.clone(),
        repo: res.native_path.clone(),
        caps: Capabilities::default(),
        console: console.clone(),
    };
    let info = res.transport.probe().await?;
    let caps = Capabilities::for_version(info.git_version);

    // One rev-parse call answers five questions.
    let out = probe_client
        .exec(GitReq::new(
            "rev-parse",
            [
                "rev-parse",
                "--is-inside-work-tree",
                "--show-toplevel",
                "--absolute-git-dir",
                "--git-common-dir",
                "--is-bare-repository",
            ],
        ))
        .await
        .map_err(|e| match e {
            GitError::CommandFailed { stderr, .. }
                if stderr.to_ascii_lowercase().contains("not a git repository") =>
            {
                GitError::NotARepository {
                    path: res.native_path.0.clone(),
                }
            }
            other => other,
        })?;
    let text = String::from_utf8_lossy(&out.stdout);
    let mut lines = text.lines().map(str::trim);
    let inside = lines.next().unwrap_or("false") == "true";
    let toplevel = lines.next().unwrap_or("").to_string();
    let git_dir = lines.next().unwrap_or("").to_string();
    let common_dir = lines.next().unwrap_or("").to_string();
    let is_bare = lines.next().unwrap_or("false") == "true";
    if !inside && !is_bare {
        return Err(GitError::NotARepository {
            path: res.native_path.0.clone(),
        });
    }
    let is_linked_worktree = !common_dir.is_empty() && common_dir != "." && common_dir != git_dir
        && !git_dir.ends_with(common_dir.trim_start_matches("./"));

    let mut warnings = res.warnings.clone();
    warnings.extend(info.warnings.clone());

    let host_path = res.transport.paths().to_host(&res.native_path).ok().map(|h| h.0);
    let repo_info = RepoInfo {
        key: entry.key.clone(),
        display_name: display_name_for(entry, &res.native_path),
        host_path,
        native_path: res.native_path.0.clone(),
        transport: res.resolved.clone(),
        git_version: info.git_version.to_string(),
        toplevel,
        git_dir,
        common_dir,
        is_bare,
        is_linked_worktree,
        warnings,
    };

    let client = GitClient {
        transport: res.transport.clone(),
        repo: res.native_path.clone(),
        caps,
        console,
    };

    Ok(Arc::new(RepoHandle {
        key: entry.key.clone(),
        info: repo_info,
        transport: res.transport,
        client,
        host_path: res.host_path,
        native_path: res.native_path,
        lock: RwLock::new(()),
        write_gate: tokio::sync::Mutex::new(()),
        operations: std::sync::Mutex::new(Default::default()),
        read_slots: Semaphore::new(4),
        cancel: CancellationToken::new(),
        graph: arc_swap::ArcSwapOption::from(None),
        graph_version: std::sync::atomic::AtomicU64::new(0),
        last_status: RwLock::new(None),
        watcher: std::sync::Mutex::new(None),
        activity: tokio::sync::watch::Sender::new(crate::watch::Activity::Focused),
        catfile: Arc::new(crate::git::catfile::CatFile::new(settings.console.verbose_helpers)),
        hidden: std::sync::RwLock::new(Default::default()),
    }))
}
