//! Typed events emitted to the frontend. Payload types derive `specta::Type`
//! so `bindings.ts` carries them.

use serde::{Deserialize, Serialize};
use specta::Type;
use tauri_specta::Event;

use crate::settings::Settings;

#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
#[serde(rename_all = "camelCase")]
pub struct SettingsChanged {
    pub revision: u64,
    pub settings: Settings,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
#[serde(rename_all = "camelCase")]
pub struct UiStateChanged {
    pub revision: u64,
}

/// Non-fatal problems the UI shows as a toast (corrupt config file, etc.).
#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
#[serde(rename_all = "camelCase")]
pub struct AppWarning {
    pub message: String,
}
