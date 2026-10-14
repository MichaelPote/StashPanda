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

/// One progress update from a running operation (a pull): the text Git wrote on stderr since its
/// last `\r` or `\n`, such as `Receiving objects:  45% (450/1000), 1.2 MiB | 500 KiB/s`.
#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
#[serde(rename_all = "camelCase")]
pub struct OperationProgress {
    pub repo_id: String,
    pub op_id: String,
    pub line: String,
    /// False once the operation has reached a step that `opCancel` cannot stop.
    pub cancellable: bool,
}

/// One progress update from a running clone, which has no repository yet to carry it, as
/// `OperationProgress` does for the operations of an open one.
#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
#[serde(rename_all = "camelCase")]
pub struct CloneProgress {
    pub op_id: String,
    pub line: String,
    /// False once the clone has reached the step that writes the files out, which `cloneCancel`
    /// cannot stop.
    pub cancellable: bool,
}
