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
    pub commit: CommitSettings,
    pub diff: DiffSettings,
    pub keys: KeySettings,
    pub library: RepoLibrary,
    /// The toolbar's items from left to right, by id (`fetch`, `commit`, `divider`, `spacer`;
    /// the list is in `src/lib/toolbarItems.ts`). None draws the default toolbar, which is also
    /// what a settings file from before the toolbar could be customised gets.
    #[serde(default)]
    pub toolbar: Option<Vec<String>>,
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
            commit: CommitSettings::default(),
            diff: DiffSettings::default(),
            keys: KeySettings::default(),
            library: RepoLibrary::default(),
            toolbar: None,
        }
    }
}

/// The SSH key manager.
#[derive(Debug, Clone, Default, Serialize, Deserialize, Type)]
#[serde(default, rename_all = "camelCase")]
pub struct KeySettings {
    /// Folders scanned for keys besides the default ones (`~/.ssh`, and the other side of the
    /// Windows and WSL boundary). Added on the Settings dialog's SSH Keys page.
    pub extra_dirs: Vec<String>,
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

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum GraphOrder {
    #[default]
    Date,
    Topo,
}

/// How the commit graph colours a lane that carries several branch tips, as when one branch was
/// started from the tip of another.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum CommitLaneColours {
    /// The lane keeps one colour. Its top-most branch tip shares it; each branch tip further down
    /// the lane has been built on since, so its pills and its Branches panel tag get a colour of
    /// their own. In both modes a remote branch that a local branch tracks takes that local
    /// branch's colour and does not count as a tip.
    #[default]
    KeepLane,
    /// The lane changes colour at each branch tip below the first, so each tip's pills, tag and
    /// the stretch of lane under it share one colour.
    ChangeAtEachTip,
}

/// The appearance settings that change how the graph snapshot is built, and the repository's
/// first lane colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GraphOptions {
    pub order: GraphOrder,
    pub lane_colours: CommitLaneColours,
    /// The palette index the automatic lane colours start from: the repository's own colour, or
    /// cyan; see `RepoHandle::graph_options`.
    pub first_colour: u8,
}

impl Default for GraphOptions {
    fn default() -> GraphOptions {
        GraphOptions {
            order: GraphOrder::default(),
            lane_colours: CommitLaneColours::default(),
            first_colour: crate::graph::lanes::FIRST_COLOUR,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum AvatarSource {
    Initials,
    Gravatar,
}

/// Which files the Commit dialog ticks when nothing is selected in the file list.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum CommitPreselect {
    /// Every changed file: staged, unstaged and untracked (not conflicted).
    #[default]
    All,
    /// Only files with staged changes, committed as they are staged.
    Staged,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, Type)]
#[serde(default, rename_all = "camelCase")]
pub struct CommitSettings {
    pub preselect: CommitPreselect,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(default, rename_all = "camelCase")]
pub struct Appearance {
    pub theme: Theme,
    /// The whole window's zoom, in percent: everything is drawn this much larger or smaller,
    /// like a browser's zoom. 100 by default; the front end keeps it between 50 and 200.
    pub zoom: u16,
    pub date_format: DateFormat,
    pub graph_order: GraphOrder,
    pub commit_lane_colours: CommitLaneColours,
    /// Swap the ref pill styles in the commit graph: remote branches solid and local branches
    /// outlined. Off (the default) draws local branches solid and remote branches outlined.
    pub solid_remote_pills: bool,
    /// Draw the commit graph's lanes at the start of the commit message column instead of in a
    /// column of their own, so each message starts just past the lanes drawn on its row and the
    /// messages indent as the number of lanes changes down the list.
    pub graph_in_message_column: bool,
    /// The row of column headings above the commits ("Commit message", "Author", "Date", "ID"),
    /// which also holds the handles for resizing the columns. On by default.
    pub show_column_headings: bool,
    /// Show only each author's picture (or initials) in the commit list, without the name; the
    /// picture's tooltip has the name and email address. Off by default.
    pub author_avatar_only: bool,
    /// Which of the commit list's columns show; the commit message always does, and takes the
    /// room the others leave. All on by default.
    pub show_graph_column: bool,
    pub show_author_column: bool,
    pub show_date_column: bool,
    pub show_id_column: bool,
    pub show_remote_branches: bool,
    pub show_stashes_in_graph: bool,
    pub avatar_source: AvatarSource,
}

impl Default for Appearance {
    fn default() -> Self {
        Self {
            theme: Theme::Dark,
            zoom: 100,
            date_format: DateFormat::Relative,
            graph_order: GraphOrder::Date,
            commit_lane_colours: CommitLaneColours::KeepLane,
            solid_remote_pills: false,
            graph_in_message_column: false,
            show_column_headings: true,
            author_avatar_only: false,
            show_graph_column: true,
            show_author_column: true,
            show_date_column: true,
            show_id_column: true,
            show_remote_branches: true,
            show_stashes_in_graph: false,
            avatar_source: AvatarSource::Initials,
        }
    }
}

impl Appearance {
    pub fn graph_options(&self) -> GraphOptions {
        GraphOptions { order: self.graph_order, lane_colours: self.commit_lane_colours, ..GraphOptions::default() }
    }
}

/// How the diff viewer draws text. The font applies to the diff only; hashes, paths and refs
/// elsewhere keep the bundled JetBrains Mono.
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(default, rename_all = "camelCase")]
pub struct DiffSettings {
    /// CSS font family name: one of the bundled fonts ("JetBrains Mono", "IBM Plex Mono") or a
    /// monospaced font installed on the machine (see `fonts_monospace`).
    pub font_family: String,
    /// Text size in CSS pixels. Before this setting the diff was fixed at 10.5 (0.75rem at a 14px root).
    pub font_size: f32,
    /// Row height as a multiple of the font size (1.2 = 20% more than the text).
    pub line_height: f32,
    /// Columns a tab advances to.
    pub tab_width: u32,
    /// Let fonts that have them join character pairs such as `=>` and `!=` into one glyph.
    pub ligatures: bool,
    /// Mark the changed words inside a changed line, not only the whole line.
    pub word_highlight: bool,
    /// Colour the code by its language, chosen from the file's extension.
    pub syntax_highlight: bool,
    /// Inside a block of changed lines, pair each removed line with the added line most like it,
    /// rather than with the one in the same position.
    pub match_similar_lines: bool,
    /// How the diff font's edges are smoothed. Only the diff text follows this; the rest of the
    /// interface keeps the system's own rendering.
    pub font_smoothing: FontSmoothing,
}

/// How the diff text is antialiased (see `src/lib/diff/font.ts` for how each is achieved).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum FontSmoothing {
    /// Subpixel antialiasing: ClearType on Windows, when it is switched on in Windows.
    #[default]
    ClearType,
    /// Grayscale antialiasing: softer edges with no colour fringes.
    Grayscale,
    /// No antialiasing: every pixel is either text or background.
    None,
}

impl Default for DiffSettings {
    fn default() -> Self {
        Self {
            font_family: "JetBrains Mono".into(),
            font_size: 12.0,
            line_height: 1.2,
            tab_width: 8,
            ligatures: true,
            word_highlight: true,
            syntax_highlight: true,
            match_similar_lines: true,
            font_smoothing: FontSmoothing::ClearType,
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
    /// A hue chosen for the repository (palette index 0 to 11): the colour of its icon in the
    /// Repositories list, the interface's accent colour while its tab is the active one, and the
    /// first automatic lane colour in its graph. Without one, the lanes start from cyan.
    pub colour: Option<u8>,
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

/// Moves keys that older versions of `settings.json` kept elsewhere, before the file is read into
/// `Settings`. Unknown keys are otherwise dropped, which would lose the user's value.
pub fn migrate_settings(json: &mut serde_json::Value) {
    // `appearance.diffLineHeight` became `diff.lineHeight` on 2026-09-30.
    let old = json
        .get_mut("appearance")
        .and_then(|a| a.as_object_mut())
        .and_then(|a| a.remove("diffLineHeight"));
    if let (Some(old), Some(root)) = (old, json.as_object_mut()) {
        let diff = root.entry("diff").or_insert_with(|| serde_json::Value::Object(Default::default()));
        if let Some(diff) = diff.as_object_mut() {
            diff.entry("lineHeight").or_insert(old);
        }
    }
}

/// Read a JSON file into `T`. A missing file yields `T::default()`. A corrupt
/// file is renamed to `<name>.broken-<unix time>` and `T::default()` is used,
/// so a bad edit never blocks start-up; the caller reports the rename.
pub fn load_json_or_default<T: Default + for<'de> Deserialize<'de>>(
    path: &Path,
) -> (T, Option<String>) {
    load_json_migrated(path, |_| {})
}

/// `load_json_or_default`, with `migrate` applied to the parsed JSON before it is read into `T`.
pub fn load_json_migrated<T: Default + for<'de> Deserialize<'de>>(
    path: &Path,
    migrate: impl FnOnce(&mut serde_json::Value),
) -> (T, Option<String>) {
    match std::fs::read(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (T::default(), None),
        Err(e) => (
            T::default(),
            Some(format!("could not read {}: {e}", path.display())),
        ),
        Ok(bytes) => match serde_json::from_slice::<serde_json::Value>(&bytes).and_then(|mut json| {
            migrate(&mut json);
            serde_json::from_value::<T>(json)
        }) {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrate_moves_diff_line_height() {
        let mut json = serde_json::json!({ "appearance": { "theme": "dark", "diffLineHeight": 1.5 } });
        migrate_settings(&mut json);
        let settings: Settings = serde_json::from_value(json).unwrap();
        assert_eq!(settings.diff.line_height, 1.5);
        assert_eq!(settings.appearance.theme, Theme::Dark);
        assert_eq!(settings.diff.font_family, "JetBrains Mono");
    }

    #[test]
    fn migrate_keeps_a_new_line_height() {
        let mut json = serde_json::json!({ "appearance": { "diffLineHeight": 1.5 }, "diff": { "lineHeight": 1.3 } });
        migrate_settings(&mut json);
        assert_eq!(json["diff"]["lineHeight"], 1.3);
        assert!(json["appearance"].get("diffLineHeight").is_none());
    }

    #[test]
    fn toolbar_layout_is_saved_and_reset() {
        // A settings file from before the toolbar could be customised has no `toolbar` key.
        let old: Settings = serde_json::from_value(serde_json::json!({ "appearance": { "theme": "light" } })).unwrap();
        assert_eq!(old.toolbar, None);

        let saved = apply_patch(&old, &serde_json::json!({ "toolbar": ["fetch", "divider", "spacer", "commit"] })).unwrap();
        assert_eq!(saved.toolbar.as_deref(), Some(&["fetch".to_string(), "divider".into(), "spacer".into(), "commit".into()][..]));

        // Reset to Default sends null, which removes the key.
        let reset = apply_patch(&saved, &serde_json::json!({ "toolbar": null })).unwrap();
        assert_eq!(reset.toolbar, None);
    }
}
