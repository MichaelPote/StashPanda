//! Settings and UI-state commands. Rust is the source of truth: the frontend
//! sends a JSON merge patch and replaces its copy from the returned document.

use tauri::{AppHandle, State};
use tauri_specta::Event;

use crate::error::Result;
use crate::ipc::events::{SettingsChanged, UiStateChanged};
use crate::settings::{apply_patch, Settings};
use crate::state::AppState;
use crate::ui_state::UiState;

#[tauri::command]
#[specta::specta]
pub async fn settings_get(state: State<'_, AppState>) -> Result<Settings> {
    Ok(state.settings())
}

#[tauri::command]
#[specta::specta]
pub async fn settings_update(
    app: AppHandle,
    state: State<'_, AppState>,
    patch: String,
) -> Result<Settings> {
    let current = state.settings();
    let patch: serde_json::Value = serde_json::from_str(&patch)?;
    let next = apply_patch(&current, &patch)?;
    let revision = state.set_settings(next.clone())?;
    SettingsChanged {
        revision,
        settings: next.clone(),
    }
    .emit(&app)?;
    Ok(next)
}

#[tauri::command]
#[specta::specta]
pub async fn ui_state_get(state: State<'_, AppState>) -> Result<UiState> {
    Ok(state.ui_state())
}

#[tauri::command]
#[specta::specta]
pub async fn ui_state_update(
    app: AppHandle,
    state: State<'_, AppState>,
    patch: String,
) -> Result<UiState> {
    let current = state.ui_state();
    let patch: serde_json::Value = serde_json::from_str(&patch)?;
    let next = apply_patch(&current, &patch)?;
    let revision = state.set_ui_state(next.clone())?;
    UiStateChanged { revision }.emit(&app)?;
    Ok(next)
}

/// Problems found while loading config files; the UI calls this once at start.
#[tauri::command]
#[specta::specta]
pub async fn startup_warnings(state: State<'_, AppState>) -> Result<Vec<String>> {
    Ok(state.startup_warnings.clone())
}
