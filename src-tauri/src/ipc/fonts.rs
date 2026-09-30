//! Fonts installed on the machine, for the Settings dialog's diff font list.

use std::collections::BTreeSet;

use crate::error::{GitError, Result};

/// Family names of the monospaced fonts installed on this machine, sorted and without repeats.
///
/// The backend and the WebView run on the same machine, so every name here can be drawn by the
/// frontend. A face counts as monospaced when its `post` table sets `isFixedPitch`, which is
/// what `fontdb` reports. Scanning reads the header of every installed font, so it runs off the
/// async runtime; the frontend asks once per run.
#[tauri::command]
#[specta::specta]
pub async fn fonts_monospace() -> Result<Vec<String>> {
    tokio::task::spawn_blocking(|| {
        let mut db = fontdb::Database::new();
        db.load_system_fonts();
        let families: BTreeSet<String> = db
            .faces()
            .filter(|face| face.monospaced)
            .filter_map(|face| face.families.first().map(|(name, _)| name.trim().to_string()))
            // macOS keeps system-only faces under names starting with a dot.
            .filter(|name| !name.is_empty() && !name.starts_with('.'))
            .collect();
        families.into_iter().collect()
    })
    .await
    .map_err(|e| GitError::internal(format!("font scan panicked: {e}")))
}

