//! The main window's web view behaves like an application window, not a browser.

use std::path::PathBuf;

use tauri::{AppHandle, Manager, WebviewWindowBuilder};

/// The main window's label, as `tauri.conf.json` and the capabilities name it.
const MAIN_WINDOW: &str = "main";

/// Creates the main window from its entry in `tauri.conf.json`. That entry has `"create": false`,
/// so Tauri leaves the window to this function, which can then choose the folder WebView2 keeps
/// its data in (see `data_folders`). When the window cannot be created with the first folder, the
/// next is tried, so that a folder another copy of StashPanda holds in a way this one cannot
/// share does not stop the app from starting.
pub fn create_main_window(app: &AppHandle) -> tauri::Result<()> {
    let config = app.config().app.windows.iter().find(|w| w.label == MAIN_WINDOW).cloned().unwrap_or_default();
    let folders = data_folders(app);
    let Some((last, earlier)) = folders.split_last() else {
        // Not WebView2: the web view keeps its data where Tauri puts it.
        WebviewWindowBuilder::from_config(app, &config)?.build()?;
        return Ok(());
    };
    for folder in earlier {
        match WebviewWindowBuilder::from_config(app, &config)?.data_directory(folder.clone()).build() {
            Ok(_) => {
                remove_unused_data_folders(app, folder.clone());
                return Ok(());
            }
            Err(e) => tracing::warn!("could not create the window with the web view data folder {}: {e}; trying another folder", folder.display()),
        }
    }
    WebviewWindowBuilder::from_config(app, &config)?.data_directory(last.clone()).build()?;
    remove_unused_data_folders(app, last.clone());
    Ok(())
}

/// The folders to try for WebView2's data, in order. Empty off Windows.
///
/// WebView2 lets several processes share one data folder only when they all run the same version
/// of the WebView2 runtime with the same browser arguments. The runtime updates itself about
/// once a month, and a copy of StashPanda left open keeps running the old version, so a second
/// copy started after an update, which gets the new version, could not create its web view in the
/// folder the first one held, and the app ended before its window showed. So the folder is named
/// after the runtime version: `webview\<version>` under the app's local data folder, and
/// `webview\dev-<version>` for a debug build, which then never shares one with an installed
/// copy. The second folder belongs to this process alone, for whatever else may stop two copies
/// sharing the first.
fn data_folders(app: &AppHandle) -> Vec<PathBuf> {
    if !cfg!(windows) {
        return Vec::new();
    }
    let Ok(local) = app.path().app_local_data_dir() else {
        return Vec::new();
    };
    let name = data_folder_name(tauri::webview_version().ok().as_deref(), cfg!(debug_assertions));
    let root = local.join(DATA_ROOT);
    vec![root.join(&name), root.join(format!("{name}-{}", std::process::id()))]
}

/// The folder under the app's local data folder that holds every WebView2 data folder.
const DATA_ROOT: &str = "webview";

/// `158.0.4370.33`, or `dev-158.0.4370.33` for a debug build. Characters a version does not
/// have are left out, since this becomes a folder name.
fn data_folder_name(runtime_version: Option<&str>, debug: bool) -> String {
    let version: String = runtime_version.unwrap_or_default().chars().filter(|c| c.is_ascii_alphanumeric() || *c == '.').collect();
    let version = if version.is_empty() { "unknown".to_string() } else { version };
    if debug {
        format!("dev-{version}")
    } else {
        version
    }
}

/// Removes, in the background, the WebView2 data folders that no running copy of StashPanda
/// uses: those of earlier runtime versions, those a single process had to itself, and
/// `EBWebView`, where the data was before the folders were named after the runtime version.
/// `in_use` is this process's own. Nothing the app needs is in any of them: its state is in
/// settings.json and the SQLite database.
fn remove_unused_data_folders(app: &AppHandle, in_use: PathBuf) {
    let Ok(local) = app.path().app_local_data_dir() else { return };
    std::thread::spawn(move || {
        let mut folders = vec![local.join("EBWebView")];
        if let Ok(entries) = std::fs::read_dir(local.join(DATA_ROOT)) {
            folders.extend(entries.flatten().map(|e| e.path()).filter(|p| p.is_dir()));
        }
        for folder in folders.into_iter().filter(|f| *f != in_use) {
            if folder.exists() {
                remove_if_unused(&folder);
            }
        }
    });
}

/// Deletes `folder` unless a running process has a file in it open. The folder is renamed
/// first: Windows refuses to rename a folder while any file in it is open, so a folder in use
/// is left whole, where deleting file by file would have taken the files that happened not to
/// be open out from under the process using it.
fn remove_if_unused(folder: &std::path::Path) {
    let Some(name) = folder.file_name() else { return };
    let doomed = folder.with_file_name(format!("{}.removing-{}", name.to_string_lossy(), std::process::id()));
    if std::fs::rename(folder, &doomed).is_err() {
        return;
    }
    if let Err(e) = std::fs::remove_dir_all(&doomed) {
        tracing::debug!("could not remove the unused web view data folder {}: {e}", doomed.display());
    }
}

/// Turns off the browser's own keyboard shortcuts in the main window, so Ctrl+F does not open a
/// find bar, F5 and Ctrl+R do not reload the page, and Ctrl+P does not print it. Only WebView2,
/// on Windows, has such shortcuts; the keys still reach the page, and the native menu's own
/// shortcuts (F5 for Refresh, Ctrl+F for Find Commit) are unaffected. The browser's developer
/// tools shortcuts go too: debug builds open them from View > Open Dev Inspector instead.
/// wry has a setting for this (`with_browser_accelerator_keys`), but Tauri 2.12 does not pass it
/// through, so it is made on WebView2 directly.
pub fn disable_browser_shortcuts(app: &AppHandle) {
    #[cfg(windows)]
    {
        use webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2Settings3;
        use windows_core::Interface;

        let Some(window) = app.get_webview_window("main") else {
            return;
        };
        let result = window.with_webview(|webview| {
            // SAFETY: the controller is the live WebView2 controller of this window, and this
            // closure runs on the thread that owns it.
            let set = || -> windows_core::Result<()> {
                unsafe {
                    let settings = webview.controller().CoreWebView2()?.Settings()?;
                    settings.cast::<ICoreWebView2Settings3>()?.SetAreBrowserAcceleratorKeysEnabled(false)
                }
            };
            if let Err(e) = set() {
                tracing::warn!("could not turn off the web view's browser shortcuts: {e}");
            }
        });
        if let Err(e) = result {
            tracing::warn!("could not reach the main window's web view: {e}");
        }
    }
    #[cfg(not(windows))]
    let _ = app;
}

/// Empties the main window's browser caches at start-up. Debug builds only: they load the page
/// from the Vite dev server as hundreds of separate modules, and every hot reload caches new
/// copies of them and their compiled code, which grew WebView2's profile to 480 MB. A release
/// build loads its files through Tauri's own protocol, which the HTTP cache does not keep.
/// Nothing the app needs lives there: its state is in settings.json and the SQLite database.
#[cfg(debug_assertions)]
pub fn clear_dev_browsing_data(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        if let Err(e) = window.clear_all_browsing_data() {
            tracing::warn!("could not clear the web view's browsing data: {e}");
        }
    }
}

/// Opens the web inspector of the main window. Debug builds only; View > Open Dev Inspector.
#[cfg(debug_assertions)]
pub fn open_inspector(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        window.open_devtools();
    }
}

/// Reloads the interface, as F5 would in a browser (the window's own shortcuts are off). The
/// backend keeps its open repositories; the frontend opens its saved tabs again. Debug builds
/// only; Developer > Refresh Window.
#[cfg(debug_assertions)]
pub fn reload_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        if let Err(e) = window.reload() {
            tracing::warn!("could not reload the window: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_data_folder_is_named_after_the_runtime_version() {
        assert_eq!(data_folder_name(Some("158.0.4370.33"), false), "158.0.4370.33");
        assert_eq!(data_folder_name(Some("158.0.4370.33"), true), "dev-158.0.4370.33");
        // WebView2 reports a preview channel as `159.0.1 beta`; nothing that cannot be in a folder name is kept.
        assert_eq!(data_folder_name(Some("159.0.1 beta/..\\x"), false), "159.0.1beta..x");
        assert_eq!(data_folder_name(None, false), "unknown");
        assert_eq!(data_folder_name(Some(""), true), "dev-unknown");
    }

    #[test]
    fn an_unused_folder_is_removed_whole() {
        let root = std::env::temp_dir().join(format!("stashpanda-webview-{}", uuid::Uuid::new_v4().simple()));
        let folder = root.join("157.0.1");
        std::fs::create_dir_all(folder.join("EBWebView")).unwrap();
        std::fs::write(folder.join("EBWebView/lockfile"), "").unwrap();
        remove_if_unused(&folder);
        assert!(!folder.exists());
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 0, "nothing is left beside it");
        // A folder that is not there is not an error.
        remove_if_unused(&folder);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Windows refuses to rename a folder with an open file in it, which is what keeps a folder
    /// another copy of the app is using whole.
    #[cfg(windows)]
    #[test]
    fn a_folder_with_an_open_file_is_left_whole() {
        let root = std::env::temp_dir().join(format!("stashpanda-webview-{}", uuid::Uuid::new_v4().simple()));
        let folder = root.join("157.0.1");
        std::fs::create_dir_all(folder.join("EBWebView")).unwrap();
        std::fs::write(folder.join("EBWebView/other"), "").unwrap();
        let held = std::fs::File::create(folder.join("EBWebView/lockfile")).unwrap();
        remove_if_unused(&folder);
        assert!(folder.join("EBWebView/other").exists() && folder.join("EBWebView/lockfile").exists());
        drop(held);
        remove_if_unused(&folder);
        assert!(!folder.exists());
        let _ = std::fs::remove_dir_all(&root);
    }
}
