//! `settings.json`: preferences and the repository library.
//!
//! Human-editable, diff-friendly, and copied between machines. Written by the
//! persist task (see `persist.rs`), never directly.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};
use specta::Type;

use crate::error::{GitError, Result};

pub const SETTINGS_FILE_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    pub version: u32,
    pub git: GitSettings,
    pub appearance: Appearance,
    pub console: ConsoleSettings,
    pub polling: Polling,
    pub cache: CacheSettings,
    pub library: RepoLibrary,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            version: SETTINGS_FILE_VERSION,
            git: GitSettings::default(),
            appearance: Appearance::default(),
            console: ConsoleSettings::default(),
            polling: Polling::default(),
            cache: CacheSettings::default(),
            library: RepoLibrary::default(),
        }
    }
}

/// Which git executable to run when a repository has no override.
#[derive(Debug, Clone, Default, Serialize, Deserialize, Type)]
#[serde(default, rename_all = "camelCase")]
pub struct GitSettings {
    /// Full path to `git.exe` used by the Windows build (None = discover).
    pub windows_executable: Option<String>,
    /// Full path to `git` used by the Linux build (None = discover).
    pub linux_executable: Option<String>,
    /// Full path to Windows `git.exe` as seen from Linux (`/mnt/c/...`), used
    /// for `/mnt/<d>/...` repositories. None = discover.
    pub windows_executable_from_linux: Option<String>,
    /// Distro used when a WSL path does not name one.
    pub wsl_default_distro: Option<String>,
    /// Automount root inside WSL (`/mnt` unless `/etc/wsl.conf` changes it).
    pub wsl_automount_root: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum Theme {
    System,
    Dark,
    Light,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum DateFormat {
    Relative,
    Absolute,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum GraphOrder {
    Date,
    Topo,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum AvatarSource {
    Initials,
    Gravatar,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(default, rename_all = "camelCase")]
pub struct Appearance {
    pub theme: Theme,
    pub date_format: DateFormat,
    pub graph_order: GraphOrder,
    pub show_remote_branches: bool,
    pub show_stashes_in_graph: bool,
    pub avatar_source: AvatarSource,
}

impl Default for Appearance {
    fn default() -> Self {
        Self {
            theme: Theme::Dark,
            date_format: DateFormat::Relative,
            graph_order: GraphOrder::Date,
            show_remote_branches: true,
            show_stashes_in_graph: false,
            avatar_source: AvatarSource::Initials,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(default, rename_all = "camelCase")]
pub struct ConsoleSettings {
    /// Show every command, including internal queries and watcher polls.
    pub show_internal: bool,
    /// Log each request sent to pinned helpers (cat-file, inotifywait).
    pub verbose_helpers: bool,
    /// Ring buffer size per repository in the UI.
    pub max_lines: u32,
    /// Bytes of stdout and stderr kept per entry.
    pub output_cap_bytes: u32,
}

impl Default for ConsoleSettings {
    fn default() -> Self {
        Self {
            show_internal: true,
            verbose_helpers: false,
            max_lines: 2000,
            output_cap_bytes: 64 * 1024,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(default, rename_all = "camelCase")]
pub struct Polling {
    pub status_interval_ms: u64,
    pub status_background_interval_ms: u64,
    pub refs_interval_ms: u64,
    pub auto_fetch_minutes: Option<u32>,
}

impl Default for Polling {
    fn default() -> Self {
        Self {
            status_interval_ms: 3000,
            status_background_interval_ms: 15000,
            refs_interval_ms: 10000,
            auto_fetch_minutes: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(default, rename_all = "camelCase")]
pub struct CacheSettings {
    pub max_commits_per_repo: u32,
    pub max_cached_repos: u32,
}

impl Default for CacheSettings {
    fn default() -> Self {
        Self {
            max_commits_per_repo: 200_000,
            max_cached_repos: 20,
        }
    }
}

/// The Repositories tree shown in the left panel, stored flat: groups form a
/// tree through `parent_id`, repositories hang off a group through `group_id`
/// (None = top level). Flat storage keeps the JSON easy to edit and avoids a
/// recursive enum, which the TypeScript exporter cannot handle.
#[derive(Debug, Clone, Default, Serialize, Deserialize, Type)]
#[serde(default, rename_all = "camelCase")]
pub struct RepoLibrary {
    pub groups: Vec<LibraryGroup>,
    pub repos: Vec<RepoEntry>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, Type)]
#[serde(default, rename_all = "camelCase")]
pub struct LibraryGroup {
    pub id: String,
    pub name: String,
    pub parent_id: Option<String>,
    pub collapsed: bool,
}

/// How a repository is reached. `Auto` resolves from the path on every open.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum Connection {
    #[default]
    Auto,
    Local,
    Wsl {
        distro: String,
    },
    Ssh {
        host: String,
        user: Option<String>,
        port: Option<u16>,
        identity_file: Option<String>,
        remote_git: Option<String>,
    },
}

/// The transport `Auto` resolved to last time; shown in the UI and used to key
/// the cache.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum ResolvedTransport {
    Local { git_executable: String },
    Wsl { distro: String },
    Ssh { host: String, user: Option<String>, port: Option<u16> },
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, Type)]
#[serde(default, rename_all = "camelCase")]
pub struct RepoEntry {
    /// Stable id: sha1 of (resolved transport | canonical native path).
    pub key: String,
    /// Path exactly as the user entered it (Windows, UNC, Linux or `user@host:/path`).
    pub path: String,
    pub connection: Connection,
    pub resolved_transport: Option<ResolvedTransport>,
    pub display_name: Option<String>,
    /// Per-repository override of the git executable.
    pub git_executable: Option<String>,
    pub group_id: Option<String>,
    pub last_opened_at: Option<i64>,
}

impl RepoLibrary {
    pub fn find(&self, key: &str) -> Option<&RepoEntry> {
        self.repos.iter().find(|r| r.key == key)
    }

    pub fn find_mut(&mut self, key: &str) -> Option<&mut RepoEntry> {
        self.repos.iter_mut().find(|r| r.key == key)
    }

    pub fn remove(&mut self, key: &str) -> Option<RepoEntry> {
        let pos = self.repos.iter().position(|r| r.key == key)?;
        Some(self.repos.remove(pos))
    }

    /// Insert or replace by key.
    pub fn upsert(&mut self, entry: RepoEntry) {
        match self.find_mut(&entry.key) {
            Some(slot) => *slot = entry,
            None => self.repos.push(entry),
        }
    }
}

// ---------------------------------------------------------------------------
// Loading, saving, patching
// ---------------------------------------------------------------------------

/// Read a JSON file into `T`. A missing file yields `T::default()`. A corrupt
/// file is renamed to `<name>.broken-<unix time>` and `T::default()` is used,
/// so a bad edit never blocks start-up; the caller reports the rename.
pub fn load_json_or_default<T: Default + for<'de> Deserialize<'de>>(
    path: &Path,
) -> (T, Option<String>) {
    match std::fs::read(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (T::default(), None),
        Err(e) => (
            T::default(),
            Some(format!("could not read {}: {e}", path.display())),
        ),
        Ok(bytes) => match serde_json::from_slice::<T>(&bytes) {
            Ok(v) => (v, None),
            Err(e) => {
                let stamp = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                let broken = path.with_extension(format!("json.broken-{stamp}"));
                let _ = std::fs::rename(path, &broken);
                (
                    T::default(),
                    Some(format!(
                        "{} could not be parsed ({e}); it was moved to {} and defaults are in use",
                        path.display(),
                        broken.display()
                    )),
                )
            }
        },
    }
}

/// Write `value` as pretty JSON to `path` atomically: write `path.tmp`, then
/// rename over the destination, so a crash mid-write never truncates the file.
pub fn save_json_atomic<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    let text = serde_json::to_vec_pretty(value)?;
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// RFC 7396 JSON merge patch: objects merge recursively, `null` deletes a key,
/// anything else replaces the target.
pub fn json_merge_patch(target: &mut serde_json::Value, patch: &serde_json::Value) {
    use serde_json::Value;
    match patch {
        Value::Object(patch_map) => {
            if !target.is_object() {
                *target = Value::Object(serde_json::Map::new());
            }
            let target_map = target.as_object_mut().expect("object");
            for (k, v) in patch_map {
                if v.is_null() {
                    target_map.remove(k);
                } else {
                    let slot = target_map
                        .entry(k.clone())
                        .or_insert(Value::Null);
                    json_merge_patch(slot, v);
                }
            }
        }
        other => *target = other.clone(),
    }
}

/// Apply a merge patch to a serialisable value, validating the result by
/// deserialising it back. Unknown keys are dropped by `serde(default)` structs.
pub fn apply_patch<T: Serialize + for<'de> Deserialize<'de>>(
    current: &T,
    patch: &serde_json::Value,
) -> Result<T> {
    let mut json = serde_json::to_value(current)?;
    json_merge_patch(&mut json, patch);
    serde_json::from_value(json).map_err(|e| GitError::Internal {
        detail: format!("patch produced an invalid document: {e}"),
    })
}

/// Unused for now but keeps a stable, sorted map type available for callers
/// that want deterministic JSON (per-repo maps in `ui_state.rs`).
pub type SortedMap<V> = BTreeMap<String, V>;
