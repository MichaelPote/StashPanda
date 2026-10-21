//! Repository library and lifecycle commands.

use std::sync::Arc;

use tauri::{AppHandle, State};
use tauri_specta::Event;

use crate::error::{GitError, Result};
use tokio_util::sync::CancellationToken;

use crate::git::console::ConsoleRecorder;
use crate::git::versions::Capabilities;
use crate::git::GitClient;
use crate::ipc::events::{CloneProgress, SettingsChanged, UiStateChanged};
use crate::platform::Host;
use crate::repo::clone::{self, CloneFolder, CloneOutcome};
use crate::repo::open::open_repo;
use crate::repo::resolve::{repo_key, resolve, Resolution};
use crate::repo::{cancel_operation, start_operation, RepoInfo};
use crate::transport::paths::NativePath;
use crate::settings::{Connection, RepoEntry, ResolvedTransport};
use crate::state::AppState;

use serde::{Deserialize, Serialize};
use specta::Type;

/// What the Add Repository dialog shows before the user commits.
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct RepoResolution {
    pub key: String,
    pub resolved_transport: ResolvedTransport,
    pub host_path: Option<String>,
    pub native_path: String,
    pub display_name: String,
    pub warnings: Vec<String>,
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Resolve without opening: which transport, which paths, which key.
#[tauri::command]
#[specta::specta]
pub async fn repo_resolve(
    state: State<'_, AppState>,
    path: String,
    connection: Connection,
) -> Result<RepoResolution> {
    Ok(resolve_entry(&state, &path, connection)?.1)
}

/// `repo_resolve` for `path` and `connection`, with the resolution it came from.
fn resolve_entry(state: &AppState, path: &str, connection: Connection) -> Result<(Resolution, RepoResolution)> {
    let settings = state.settings();
    let entry = RepoEntry {
        path: path.trim().to_string(),
        connection,
        ..Default::default()
    };
    let res = resolve(&entry, &settings, &state.host, Some(&state.ssh))?;
    let key = repo_key(&res.resolved, &res.native_path);
    let display_name = res
        .native_path
        .0
        .trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or("")
        .to_string();
    let is_ssh = matches!(res.resolved, ResolvedTransport::Ssh { .. });
    let preview = RepoResolution {
        key,
        resolved_transport: res.resolved.clone(),
        host_path: (!is_ssh).then(|| res.host_path.0.clone()),
        native_path: res.native_path.0.clone(),
        display_name,
        warnings: res.warnings.clone(),
    };
    Ok((res, preview))
}

/// What the Clone From tab shows before the user commits: where the clone would go, and what is
/// in that folder now.
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct CloneTarget {
    pub resolution: RepoResolution,
    pub folder: CloneFolder,
}

/// Resolve a folder to clone into and look at what is in it, without changing anything. Fails
/// when the path is a file, or when neither it nor the folder that would contain it exists.
#[tauri::command]
#[specta::specta]
pub async fn clone_inspect(state: State<'_, AppState>, path: String, connection: Connection) -> Result<CloneTarget> {
    let (res, resolution) = resolve_entry(&state, &path, connection)?;
    let folder = clone::inspect(&res.transport, &res.native_path).await?;
    Ok(CloneTarget { resolution, folder })
}

/// What `repo_clone` made: the library entry, and how the clone went.
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct CloneResult {
    pub entry: RepoEntry,
    pub outcome: CloneOutcome,
}

/// Clone `url` into `path` and add the result to the library. The folder may be missing, empty
/// or already have files in it, which are kept: see `repo::clone`. A folder that is already a
/// repository is refused. `opId` is chosen by the caller: each progress update arrives as a
/// `cloneProgress` event carrying it, and `cloneCancel` with it stops the clone while it
/// downloads, which then fails with `cancelled` and leaves the folder as it was.
#[tauri::command]
#[specta::specta]
#[allow(clippy::too_many_arguments)]
pub async fn repo_clone(
    app: AppHandle,
    state: State<'_, AppState>,
    op_id: String,
    url: String,
    path: String,
    connection: Connection,
    group_id: Option<String>,
    display_name: Option<String>,
) -> Result<CloneResult> {
    let url = url.trim().to_string();
    clone::check_url(&url)?;
    let (res, preview) = resolve_entry(&state, &path, connection.clone())?;
    let folder = clone::inspect(&res.transport, &res.native_path).await?;
    let outer_path = clone::parent_native(&res.native_path).ok_or_else(|| GitError::InvalidPath {
        path: res.native_path.0.clone(),
        reason: "cannot clone into the root of a drive or filesystem".into(),
    })?;
    let info = res.transport.probe().await?;
    let console = Arc::new(ConsoleRecorder::new(Some(app.clone()), preview.key.clone(), state.settings().console.output_cap_bytes as usize));
    let client = |repo: NativePath| GitClient {
        transport: res.transport.clone(),
        repo,
        caps: Capabilities::for_version(info.git_version),
        console: console.clone(),
    };
    let (inner, outer) = (client(res.native_path.clone()), client(outer_path));
    let outcome = {
        let op = start_operation(&state.clones, &op_id, CancellationToken::new())?;
        let progress = |line: String| {
            let _ = CloneProgress { op_id: op_id.clone(), line, cancellable: op.is_cancellable() }.emit(&app);
        };
        clone::clone_into(&inner, &outer, &folder, &url, &progress, &op, &op.token).await?
    };
    let entry = repo_add(app, state, path, connection, group_id, display_name).await?;
    Ok(CloneResult { entry, outcome })
}

/// Trust a repository that Git refused to open because another user owns it (`unsafeRepository`,
/// Git's "dubious ownership"): adds `path`, exactly as the error gave it, to `safe.directory` in
/// the global config of the Git that refused. It runs through the library entry's own connection,
/// so a WSL or SSH repository gets the setting in that system's Git config. Call `repoOpen` again
/// afterwards.
#[tauri::command]
#[specta::specta]
pub async fn repo_fix_safe_directory(app: AppHandle, state: State<'_, AppState>, key: String, path: String) -> Result<crate::git::ops::OpResult> {
    let settings = state.settings();
    let entry = settings.library.find(&key).cloned().ok_or_else(|| GitError::RepoNotOpen { repo_id: key.clone() })?;
    let res = resolve(&entry, &settings, &state.host, Some(&state.ssh))?;
    let info = res.transport.probe().await?;
    let client = GitClient {
        transport: res.transport.clone(),
        repo: res.native_path.clone(),
        caps: Capabilities::for_version(info.git_version),
        console: Arc::new(ConsoleRecorder::new(Some(app), key, settings.console.output_cap_bytes as usize)),
    };
    crate::git::ops::add_safe_directory(&client, &path).await
}

/// Stop a clone started with `opId`. Resolves false when it has finished or is writing the files
/// out, which always runs to the end.
#[tauri::command]
#[specta::specta]
pub async fn clone_cancel(state: State<'_, AppState>, op_id: String) -> Result<bool> {
    Ok(cancel_operation(&state.clones, &op_id))
}

/// Add to the library (or update an existing entry with the same key).
#[tauri::command]
#[specta::specta]
pub async fn repo_add(
    app: AppHandle,
    state: State<'_, AppState>,
    path: String,
    connection: Connection,
    group_id: Option<String>,
    display_name: Option<String>,
) -> Result<RepoEntry> {
    let mut settings = state.settings();
    let mut entry = RepoEntry {
        path: path.trim().to_string(),
        connection,
        group_id,
        display_name: display_name.filter(|s| !s.trim().is_empty()),
        ..Default::default()
    };
    let res = resolve(&entry, &settings, &state.host, Some(&state.ssh))?;
    entry.key = repo_key(&res.resolved, &res.native_path);
    entry.resolved_transport = Some(res.resolved);
    if let Some(existing) = settings.library.find(&entry.key) {
        entry.last_opened_at = existing.last_opened_at;
        if entry.display_name.is_none() {
            entry.display_name = existing.display_name.clone();
        }
    }
    settings.library.upsert(entry.clone());
    let revision = state.set_settings(settings.clone())?;
    SettingsChanged { revision, settings }.emit(&app)?;
    Ok(entry)
}

/// Change a library entry's path, connection or display name. A new path or connection can give
/// the entry a new key, in which case the entry under the old key is replaced. The group, the git
/// override and the last-opened time carry over. An empty `display_name` clears the name.
#[tauri::command]
#[specta::specta]
pub async fn repo_update(
    app: AppHandle,
    state: State<'_, AppState>,
    key: String,
    path: String,
    connection: Connection,
    display_name: Option<String>,
) -> Result<RepoEntry> {
    let mut settings = state.settings();
    let existing = settings.library.find(&key).cloned().ok_or_else(|| GitError::Internal {
        detail: format!("repository {key} is not in the library"),
    })?;
    let mut entry = RepoEntry {
        path: path.trim().to_string(),
        connection,
        display_name: display_name.filter(|s| !s.trim().is_empty()),
        git_executable: existing.git_executable,
        group_id: existing.group_id,
        last_opened_at: existing.last_opened_at,
        colour: existing.colour,
        ..Default::default()
    };
    let res = resolve(&entry, &settings, &state.host, Some(&state.ssh))?;
    entry.key = repo_key(&res.resolved, &res.native_path);
    entry.resolved_transport = Some(res.resolved);
    if entry.key != key {
        if let Some(other) = settings.library.find(&entry.key) {
            return Err(GitError::InvalidPath {
                path: entry.path,
                reason: format!("it is already in the library as {}", other.path),
            });
        }
        settings.library.remove(&key);
    }
    settings.library.upsert(entry.clone());
    let revision = state.set_settings(settings.clone())?;
    SettingsChanged { revision, settings }.emit(&app)?;
    Ok(entry)
}

/// Gives a repository a colour (a hue, palette index 0 to 11), or takes it away with None. It
/// colours the repository's icon in the Repositories list, the interface's accent while its tab is
/// active, and the first automatic lane colour in its graph (cyan without one), so an open
/// repository's graph is rebuilt with it. Saved in the library in the settings, not in the
/// repository.
#[tauri::command]
#[specta::specta]
pub async fn repo_set_colour(app: AppHandle, state: State<'_, AppState>, key: String, colour: Option<u8>) -> Result<RepoEntry> {
    let mut settings = state.settings();
    let entry = settings.library.find_mut(&key).ok_or_else(|| GitError::Internal {
        detail: format!("repository {key} is not in the library"),
    })?;
    entry.colour = colour.filter(|c| *c < crate::graph::lanes::HUE_COUNT);
    let entry = entry.clone();
    let revision = state.set_settings(settings.clone())?;
    let options = settings.appearance.graph_options();
    SettingsChanged { revision, settings }.emit(&app)?;
    if let Some(h) = state.repos.read().await.get(&key).cloned() {
        h.set_first_colour(entry.colour);
        crate::graph::loader::rebuild_filtered(Some(&app), &h, options);
    }
    Ok(entry)
}

#[tauri::command]
#[specta::specta]
pub async fn repo_remove(app: AppHandle, state: State<'_, AppState>, key: String) -> Result<()> {
    if let Some(handle) = state.repos.write().await.remove(&key) {
        crate::shutdown::retire(handle.clone());
        handle.close().await;
    }
    let mut settings = state.settings();
    settings.library.remove(&key);
    let revision = state.set_settings(settings.clone())?;
    SettingsChanged { revision, settings }.emit(&app)?;
    // So does its cached graph. A failure only leaves rows that the next start evicts.
    let cache_key = key.clone();
    if let Err(e) = state.db.run(move |c| crate::storage::repo_cache::forget(c, &cache_key)).await {
        tracing::warn!("could not remove the cached graph of {key}: {e}");
    }
    // Its remembered UI state (hidden refs, last selected commit) goes with it.
    let mut ui = state.ui_state();
    if ui.per_repo.remove(&key).is_some() {
        let revision = state.set_ui_state(ui)?;
        UiStateChanged { revision }.emit(&app)?;
    }
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub async fn repo_open(app: AppHandle, state: State<'_, AppState>, key: String) -> Result<RepoInfo> {
    if let Some(existing) = state.repos.read().await.get(&key) {
        // A frontend that reloaded has lost the watcher state; announce it again.
        let last = existing.watcher_state.lock().ok().and_then(|s| s.clone());
        if let Some(ev) = last {
            let _ = ev.emit(&app);
        }
        return Ok(existing.info.clone());
    }
    let mut settings = state.settings();
    let entry = settings
        .library
        .find(&key)
        .cloned()
        .ok_or_else(|| GitError::RepoNotOpen { repo_id: key.clone() })?;
    let handle = open_repo(Some(&app), &entry, &settings, &state.host, Some(&state.ssh)).await?;
    // Refs switched off in the graph last time, saved by the frontend in the UI state.
    let hidden: crate::graph::snapshot::HiddenRefs = state
        .ui_state()
        .per_repo
        .get(&key)
        .map(|r| r.hidden_refs.iter().cloned().collect())
        .unwrap_or_default();
    *handle.hidden.write().expect("hidden refs lock") = std::sync::Arc::new(hidden);
    // Colours chosen for refs, saved the same way.
    let colours: crate::graph::snapshot::RefColours = state
        .ui_state()
        .per_repo
        .get(&key)
        .map(|r| r.ref_colours.iter().map(|(k, c)| (k.clone(), *c)).collect())
        .unwrap_or_default();
    *handle.colours.write().expect("ref colours lock") = std::sync::Arc::new(colours);
    let info = handle.info.clone();
    crate::repo::watch_start::start(&app, &state.db, &handle, &settings);
    state.repos.write().await.insert(key.clone(), handle);

    if let Some(e) = settings.library.find_mut(&key) {
        e.last_opened_at = Some(now_secs());
        e.resolved_transport = Some(info.transport.clone());
    }
    let revision = state.set_settings(settings.clone())?;
    SettingsChanged { revision, settings }.emit(&app)?;
    Ok(info)
}

#[tauri::command]
#[specta::specta]
pub async fn repo_close(state: State<'_, AppState>, key: String) -> Result<()> {
    if let Some(handle) = state.repos.write().await.remove(&key) {
        crate::shutdown::retire(handle.clone());
        handle.close().await;
    }
    Ok(())
}

/// The checked-out branch of a repository in the library, for its row in the Repositories list.
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct RepoHead {
    pub key: String,
    /// The checked-out branch's short name; None when HEAD is detached.
    pub branch: Option<String>,
    /// The short ID of the commit a detached HEAD is at; None on a branch.
    pub commit: Option<String>,
}

/// The checked-out branch of each of `keys`, read without opening them and kept out of the
/// console. A repository on an SSH server is read only while a connection to that server is
/// already open, so this never logs in or asks for a password. Repositories that are skipped or
/// cannot be read (moved, deleted) are left out.
#[tauri::command]
#[specta::specta]
pub async fn repo_heads(state: State<'_, AppState>, keys: Vec<String>) -> Result<Vec<RepoHead>> {
    let settings = state.settings();
    let mut reads = tokio::task::JoinSet::new();
    for key in keys {
        let Some(entry) = settings.library.find(&key) else { continue };
        let Ok(res) = resolve(entry, &settings, &state.host, Some(&state.ssh)) else { continue };
        reads.spawn(read_head(key, res));
    }
    let mut heads = Vec::new();
    while let Some(done) = reads.join_next().await {
        if let Ok(Some(head)) = done {
            heads.push(head);
        }
    }
    Ok(heads)
}

async fn read_head(key: String, res: Resolution) -> Option<RepoHead> {
    if !res.transport.connected().await {
        return None;
    }
    let client = GitClient {
        transport: res.transport.clone(),
        repo: res.native_path.clone(),
        caps: Capabilities::default(),
        // No app handle, so nothing reaches the console.
        console: Arc::new(ConsoleRecorder::new(None, key.clone(), 0)),
    };
    let timeout = Some(std::time::Duration::from_secs(15));
    let outs = client
        .exec_batch(vec![
            crate::git::GitReq::new("symbolic-ref", ["symbolic-ref", "-q", "--short", "HEAD"]).timeout(timeout),
            crate::git::GitReq::new("rev-parse", ["rev-parse", "-q", "--verify", "--short", "HEAD"]).timeout(timeout),
        ])
        .await
        .ok()?;
    let text = |i: usize| {
        outs.get(i)
            .filter(|o| o.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .filter(|s| !s.is_empty())
    };
    // An unborn branch still has its name; a detached HEAD has only its commit.
    let branch = text(0);
    let commit = if branch.is_some() { None } else { text(1) };
    (branch.is_some() || commit.is_some()).then_some(RepoHead { key, branch, commit })
}

#[tauri::command]
#[specta::specta]
pub async fn repo_list_open(state: State<'_, AppState>) -> Result<Vec<RepoInfo>> {
    Ok(state.repos.read().await.values().map(|h| h.info.clone()).collect())
}

#[tauri::command]
#[specta::specta]
pub async fn wsl_list_distros(state: State<'_, AppState>) -> Result<Vec<String>> {
    if !matches!(state.host, Host::Windows) {
        return Ok(Vec::new());
    }
    crate::transport::wsl::list_distros().await
}

/// Which OS this build runs on, so the UI can phrase paths and options.
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum HostInfo {
    Windows,
    LinuxWsl { distro: String },
    Linux,
    MacOs,
}

#[tauri::command]
#[specta::specta]
pub async fn host_info(state: State<'_, AppState>) -> Result<HostInfo> {
    Ok(match &state.host {
        Host::Windows => HostInfo::Windows,
        Host::LinuxWsl { distro } => HostInfo::LinuxWsl { distro: distro.clone() },
        Host::Linux => HostInfo::Linux,
        Host::MacOs => HostInfo::MacOs,
    })
}

/// Record the host key a server offered, so the next open succeeds.
#[tauri::command]
#[specta::specta]
pub async fn ssh_trust_host(state: State<'_, AppState>, host: String, port: u16) -> Result<()> {
    state.ssh.trust_offered(&host, port)
}

/// Remember a password for `user@host:port` for this session. `host`, `port` and `user` are the
/// ones an `sshAuthFailed` error gave; call `repoOpen` again afterwards. A password the server
/// refuses is forgotten, and the open fails with `sshAuthFailed` again with `passwordRejected`.
#[tauri::command]
#[specta::specta]
pub async fn ssh_set_password(state: State<'_, AppState>, host: String, port: u16, user: String, password: String) -> Result<()> {
    state.ssh.passwords.remember(&crate::transport::ssh_pool::password_key(&user, &host, port), &password);
    Ok(())
}

#[allow(dead_code)]
fn _keep(_: Arc<()>) {}
