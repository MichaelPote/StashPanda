//! Process-wide state managed by Tauri.

use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use tauri::Manager;

use crate::error::{GitError, Result};
use crate::keys::secrets::PassphraseCache;
use crate::transport::ssh_pool::SshContext;
use crate::platform::Host;
use crate::repo::{OperationRegistry, RepoHandle};
use crate::persist::{self, PersistHandle, PersistRequest, PersistSource};
use crate::settings::{load_json_migrated, load_json_or_default, migrate_settings, Settings};
use crate::storage::{repo_cache, ui_prefs, Db};
use crate::ui_state::UiState;

pub struct AppPaths {
    pub config_dir: PathBuf,
    pub data_dir: PathBuf,
    pub settings_file: PathBuf,
    /// Where the UI state lived before it moved to the database; imported once, then renamed.
    pub ui_state_file: PathBuf,
    pub cache_db: PathBuf,
}

/// Values shared by every command. `Inner` is what the persist task reads.
pub struct Inner {
    pub settings: RwLock<Settings>,
    pub ui_state: RwLock<UiState>,
    pub settings_revision: RwLock<u64>,
    pub ui_state_revision: RwLock<u64>,
}

impl PersistSource for Inner {
    fn snapshot_settings(&self) -> Settings {
        self.settings.read().expect("settings lock").clone()
    }
    fn snapshot_ui_state(&self) -> UiState {
        self.ui_state.read().expect("ui_state lock").clone()
    }
}

pub struct AppState {
    pub paths: AppPaths,
    pub inner: Arc<Inner>,
    pub db: Arc<Db>,
    pub persist: PersistHandle,
    /// Problems found while loading config files, shown once by the UI.
    pub startup_warnings: Vec<String>,
    pub host: Host,
    pub passphrases: Arc<PassphraseCache>,
    pub ssh: Arc<SshContext>,
    /// Open repositories by library key.
    pub repos: tokio::sync::RwLock<std::collections::HashMap<String, Arc<RepoHandle>>>,
    /// Running clones, by the operation id the frontend gave. A clone has no `RepoHandle` yet.
    pub clones: OperationRegistry,
}

impl AppState {
    pub fn init(app: &tauri::AppHandle) -> Result<AppState> {
        let config_dir = app.path().app_config_dir()?;
        let data_dir = app.path().app_data_dir()?;
        std::fs::create_dir_all(&config_dir)?;
        std::fs::create_dir_all(&data_dir)?;
        let paths = AppPaths {
            settings_file: config_dir.join("settings.json"),
            ui_state_file: config_dir.join("ui-state.json"),
            cache_db: data_dir.join("stashpanda.sqlite"),
            config_dir,
            data_dir,
        };

        let mut warnings = Vec::new();
        let (settings, w) = load_json_migrated::<Settings>(&paths.settings_file, migrate_settings);
        warnings.extend(w);
        let db = Db::open(&paths.cache_db)?;
        evict_cache(&db, &settings);
        let ui_state = load_ui_state(&db, &paths.ui_state_file, &mut warnings);

        let inner = Arc::new(Inner {
            settings: RwLock::new(settings),
            ui_state: RwLock::new(ui_state),
            settings_revision: RwLock::new(0),
            ui_state_revision: RwLock::new(0),
        });

        let persist = persist::spawn(paths.settings_file.clone(), db.clone(), inner.clone());

        tracing::info!(
            "config dir {}, data dir {}",
            paths.config_dir.display(),
            paths.data_dir.display()
        );

        let passphrases = Arc::new(PassphraseCache::default());
        Ok(AppState {
            paths,
            inner,
            db,
            persist,
            startup_warnings: warnings,
            host: Host::detect(),
            passphrases: passphrases.clone(),
            ssh: Arc::new(SshContext::new(passphrases, SshContext::default_known_hosts())),
            repos: tokio::sync::RwLock::new(std::collections::HashMap::new()),
            clones: Default::default(),
        })
    }

    pub fn settings(&self) -> Settings {
        self.inner.snapshot_settings()
    }

    pub fn ui_state(&self) -> UiState {
        self.inner.snapshot_ui_state()
    }

    /// Replace settings, bump the revision, queue a save. Returns the revision.
    pub fn set_settings(&self, new: Settings) -> Result<u64> {
        *self
            .inner
            .settings
            .write()
            .map_err(|_| GitError::internal("settings lock poisoned"))? = new;
        let mut rev = self
            .inner
            .settings_revision
            .write()
            .map_err(|_| GitError::internal("revision lock poisoned"))?;
        *rev += 1;
        self.persist.request(PersistRequest::Settings);
        Ok(*rev)
    }

    pub fn set_ui_state(&self, new: UiState) -> Result<u64> {
        *self
            .inner
            .ui_state
            .write()
            .map_err(|_| GitError::internal("ui_state lock poisoned"))? = new;
        let mut rev = self
            .inner
            .ui_state_revision
            .write()
            .map_err(|_| GitError::internal("revision lock poisoned"))?;
        *rev += 1;
        self.persist.request(PersistRequest::UiState);
        Ok(*rev)
    }

    pub fn flush(&self) {
        self.persist.flush_now(self.inner.as_ref());
    }
}

/// Drop the cached graphs of repositories no longer in the library, then of the least recently
/// opened beyond `cache.maxCachedRepos`. A failure only costs disk space, so it is logged.
fn evict_cache(db: &Db, settings: &Settings) {
    let keys: Vec<String> = settings.library.repos.iter().map(|r| r.key.clone()).collect();
    let max = settings.cache.max_cached_repos;
    match db.run_blocking(|c| repo_cache::evict(c, &keys, max)) {
        Ok(0) => {}
        Ok(n) => tracing::info!("evicted {n} repositories from the graph cache"),
        Err(e) => tracing::warn!("could not evict old repositories from the graph cache: {e}"),
    }
}

/// The UI state from the database. When the database has none yet, `ui-state.json` from before
/// the move is imported into it and renamed to `ui-state.json.imported`, so it is read only once.
fn load_ui_state(db: &Db, legacy_file: &std::path::Path, warnings: &mut Vec<String>) -> UiState {
    match db.run_blocking(|c| ui_prefs::load(c)) {
        Ok(Some(json)) => match serde_json::from_value::<UiState>(json) {
            Ok(state) => return state,
            Err(e) => {
                warnings.push(format!("The saved UI state could not be read ({e}); panel sizes and similar choices are back to their defaults."));
                return UiState::default();
            }
        },
        Ok(None) => {}
        Err(e) => {
            warnings.push(format!("The saved UI state could not be read from the database: {e}"));
            return UiState::default();
        }
    }
    if !legacy_file.exists() {
        return UiState::default();
    }
    let (state, w) = load_json_or_default::<UiState>(legacy_file);
    warnings.extend(w);
    let snapshot = state.clone();
    match db.run_blocking(move |c| ui_prefs::save(c, &snapshot)) {
        Ok(()) => {
            let imported = legacy_file.with_extension("json.imported");
            if let Err(e) = std::fs::rename(legacy_file, &imported) {
                tracing::warn!("imported {} but could not rename it: {e}", legacy_file.display());
            } else {
                tracing::info!("imported {} into the database", legacy_file.display());
            }
        }
        // The file stays where it is, so the next start tries again.
        Err(e) => warnings.push(format!("Could not move {} into the database: {e}", legacy_file.display())),
    }
    state
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_ui_state_file_is_imported_once() {
        let dir = std::env::temp_dir().join(format!("stashpanda-ui-import-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("ui-state.json");
        std::fs::write(&file, r#"{"panels":{"leftWidth":33.0},"perRepo":{"r1":{"hiddenRefs":["refs/heads/x"]}}}"#).unwrap();
        let db = Db::open_in_memory().unwrap();
        let mut warnings = Vec::new();

        let state = load_ui_state(&db, &file, &mut warnings);
        assert_eq!(state.panels.left_width, 33.0);
        assert!(!file.exists() && dir.join("ui-state.json.imported").exists());
        assert!(warnings.is_empty(), "{warnings:?}");

        // The second start reads the database, not the (renamed) file.
        let again = load_ui_state(&db, &file, &mut warnings);
        assert_eq!(again.per_repo["r1"].hidden_refs, vec!["refs/heads/x"]);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
