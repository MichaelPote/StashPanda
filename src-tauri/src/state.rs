//! Process-wide state managed by Tauri.

use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use tauri::Manager;

use crate::error::{GitError, Result};
use crate::keys::secrets::PassphraseCache;
use crate::transport::ssh_pool::SshContext;
use crate::platform::Host;
use crate::repo::RepoHandle;
use crate::persist::{self, PersistHandle, PersistRequest, PersistSource};
use crate::settings::{load_json_or_default, Settings};
use crate::storage::Db;
use crate::ui_state::UiState;

pub struct AppPaths {
    pub config_dir: PathBuf,
    pub data_dir: PathBuf,
    pub settings_file: PathBuf,
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
        let (settings, w) = load_json_or_default::<Settings>(&paths.settings_file);
        warnings.extend(w);
        let (ui_state, w) = load_json_or_default::<UiState>(&paths.ui_state_file);
        warnings.extend(w);

        let db = Db::open(&paths.cache_db)?;

        let inner = Arc::new(Inner {
            settings: RwLock::new(settings),
            ui_state: RwLock::new(ui_state),
            settings_revision: RwLock::new(0),
            ui_state_revision: RwLock::new(0),
        });

        let persist = persist::spawn(
            paths.settings_file.clone(),
            paths.ui_state_file.clone(),
            inner.clone(),
        );

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
