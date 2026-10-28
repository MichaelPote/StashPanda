//! The main window's web view behaves like an application window, not a browser.

use tauri::AppHandle;

/// Turns off the browser's own keyboard shortcuts in the main window, so Ctrl+F does not open a
/// find bar, F5 and Ctrl+R do not reload the page, and Ctrl+P does not print it. Only WebView2,
/// on Windows, has such shortcuts; the keys still reach the page, and the native menu's own
/// shortcuts (F5 for Refresh, Ctrl+F for Find Commit) are unaffected. The browser's developer
/// tools shortcuts go too: debug builds open them from View > Open Dev Inspector instead.
pub fn disable_browser_shortcuts(app: &AppHandle) {
    #[cfg(windows)]
    {
        use tauri::Manager;
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
    use tauri::Manager;
    if let Some(window) = app.get_webview_window("main") {
        if let Err(e) = window.clear_all_browsing_data() {
            tracing::warn!("could not clear the web view's browsing data: {e}");
        }
    }
}

/// Opens the web inspector of the main window. Debug builds only; View > Open Dev Inspector.
#[cfg(debug_assertions)]
pub fn open_inspector(app: &AppHandle) {
    use tauri::Manager;
    if let Some(window) = app.get_webview_window("main") {
        window.open_devtools();
    }
}

/// Reloads the interface, as F5 would in a browser (the window's own shortcuts are off). The
/// backend keeps its open repositories; the frontend opens its saved tabs again. Debug builds
/// only; Developer > Refresh Window.
#[cfg(debug_assertions)]
pub fn reload_window(app: &AppHandle) {
    use tauri::Manager;
    if let Some(window) = app.get_webview_window("main") {
        if let Err(e) = window.reload() {
            tracing::warn!("could not reload the window: {e}");
        }
    }
}
