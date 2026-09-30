//! Repository library and lifecycle commands.

use std::sync::Arc;

use tauri::{AppHandle, State};
use tauri_specta::Event;

use crate::error::{GitError, Result};
use crate::ipc::events::SettingsChanged;
use crate::platform::Host;
use crate::repo::open::open_repo;
use crate::repo::resolve::{repo_key, resolve};
use crate::repo::RepoInfo;
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
    Ok(RepoResolution {
        key,
        resolved_transport: res.resolved,
        host_path: (!is_ssh).then_some(res.host_path.0),
        native_path: res.native_path.0,
        display_name,
        warnings: res.warnings,
    })
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

#[tauri::command]
#[specta::specta]
pub async fn repo_remove(app: AppHandle, state: State<'_, AppState>, key: String) -> Result<()> {
    if let Some(handle) = state.repos.write().await.remove(&key) {
        handle.close().await;
    }
    let mut settings = state.settings();
    settings.library.remove(&key);
    let revision = state.set_settings(settings.clone())?;
    SettingsChanged { revision, settings }.emit(&app)?;
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub async fn repo_open(app: AppHandle, state: State<'_, AppState>, key: String) -> Result<RepoInfo> {
    if let Some(existing) = state.repos.read().await.get(&key) {
        return Ok(existing.info.clone());
    }
    let mut settings = state.settings();
    let entry = settings
        .library
        .find(&key)
        .cloned()
        .ok_or_else(|| GitError::RepoNotOpen { repo_id: key.clone() })?;
    let handle = open_repo(Some(&app), &entry, &settings, &state.host, Some(&state.ssh)).await?;
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
        handle.close().await;
    }
    Ok(())
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

/// Remember a password for `user@host:port` for this session.
#[tauri::command]
#[specta::specta]
pub async fn ssh_set_password(state: State<'_, AppState>, host: String, port: u16, user: String, password: String) -> Result<()> {
    state.ssh.passwords.remember(&format!("{user}@{host}:{port}"), &password);
    Ok(())
}

#[allow(dead_code)]
fn _keep(_: Arc<()>) {}
