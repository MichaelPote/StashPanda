//! Debounced writer for `settings.json` and `ui-state.json`.
//!
//! Callers send a [`PersistRequest`]; the task waits for the debounce window
//! to pass with no further request of that kind, then snapshots the current
//! value from `AppState` and writes it atomically. `flush_now` writes both
//! files synchronously and is called on exit.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc;
use tokio::time::Instant;

use crate::settings::{save_json_atomic, Settings};
use crate::ui_state::UiState;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PersistRequest {
    Settings,
    UiState,
}

const SETTINGS_DEBOUNCE: Duration = Duration::from_millis(500);
const UI_STATE_DEBOUNCE: Duration = Duration::from_millis(1000);

/// What the task needs to read the current values.
pub trait PersistSource: Send + Sync + 'static {
    fn snapshot_settings(&self) -> Settings;
    fn snapshot_ui_state(&self) -> UiState;
}

#[derive(Clone)]
pub struct PersistHandle {
    tx: mpsc::UnboundedSender<PersistRequest>,
    settings_path: PathBuf,
    ui_state_path: PathBuf,
}

impl PersistHandle {
    pub fn request(&self, what: PersistRequest) {
        // A closed channel only happens during shutdown; flush_now covers it.
        let _ = self.tx.send(what);
    }

    /// Synchronous write of both files. Used from `RunEvent::ExitRequested`.
    pub fn flush_now(&self, source: &dyn PersistSource) {
        if let Err(e) = save_json_atomic(&self.settings_path, &source.snapshot_settings()) {
            tracing::error!("could not save settings.json on exit: {e}");
        }
        if let Err(e) = save_json_atomic(&self.ui_state_path, &source.snapshot_ui_state()) {
            tracing::error!("could not save ui-state.json on exit: {e}");
        }
    }
}

/// Spawn the persist task on the Tauri async runtime.
pub fn spawn(
    settings_path: PathBuf,
    ui_state_path: PathBuf,
    source: Arc<dyn PersistSource>,
) -> PersistHandle {
    let (tx, mut rx) = mpsc::unbounded_channel::<PersistRequest>();
    let handle = PersistHandle {
        tx,
        settings_path: settings_path.clone(),
        ui_state_path: ui_state_path.clone(),
    };

    tauri::async_runtime::spawn(async move {
        let mut settings_due: Option<Instant> = None;
        let mut ui_due: Option<Instant> = None;

        loop {
            // Sleep until the earliest deadline, or forever if nothing is pending.
            let next = match (settings_due, ui_due) {
                (Some(a), Some(b)) => Some(a.min(b)),
                (Some(a), None) => Some(a),
                (None, Some(b)) => Some(b),
                (None, None) => None,
            };

            tokio::select! {
                msg = rx.recv() => {
                    match msg {
                        Some(PersistRequest::Settings) => {
                            settings_due = Some(Instant::now() + SETTINGS_DEBOUNCE);
                        }
                        Some(PersistRequest::UiState) => {
                            ui_due = Some(Instant::now() + UI_STATE_DEBOUNCE);
                        }
                        None => break, // all senders dropped
                    }
                }
                _ = async {
                    match next {
                        Some(t) => tokio::time::sleep_until(t).await,
                        None => std::future::pending::<()>().await,
                    }
                } => {
                    let now = Instant::now();
                    if settings_due.is_some_and(|t| t <= now) {
                        settings_due = None;
                        let snapshot = source.snapshot_settings();
                        let path = settings_path.clone();
                        if let Err(e) = tokio::task::spawn_blocking(move || save_json_atomic(&path, &snapshot)).await {
                            tracing::error!("settings persist task panicked: {e}");
                        }
                    }
                    if ui_due.is_some_and(|t| t <= now) {
                        ui_due = None;
                        let snapshot = source.snapshot_ui_state();
                        let path = ui_state_path.clone();
                        if let Err(e) = tokio::task::spawn_blocking(move || save_json_atomic(&path, &snapshot)).await {
                            tracing::error!("ui-state persist task panicked: {e}");
                        }
                    }
                }
            }
        }
    });

    handle
}
