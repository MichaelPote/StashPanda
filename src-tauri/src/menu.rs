//! The native application menu bar. Clicks are forwarded to the frontend as
//! `menu_action` events carrying the item id; the frontend owns the behaviour
//! so it can use its dialogs, stores and the active tab.

use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::menu::{CheckMenuItem, CheckMenuItemBuilder, MenuBuilder, MenuItem, MenuItemBuilder, PredefinedMenuItem, SubmenuBuilder};
use tauri::{AppHandle, Manager, Wry};
use tauri_specta::Event;

use crate::settings::Theme;
use crate::state::AppState;

#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
#[serde(rename_all = "camelCase")]
pub struct MenuActionEvent {
    /// Dotted id such as `file.open`, `view.toggle_console`, `repo.create_branch`.
    pub id: String,
}

/// The View > Theme check items, kept so their ticks can follow the `appearance.theme` setting.
struct ThemeMenu {
    dark: CheckMenuItem<Wry>,
    light: CheckMenuItem<Wry>,
    system: CheckMenuItem<Wry>,
}

/// Ticks the item for `theme` and clears the other two. A check item flips its own tick when
/// clicked, so without this every theme clicked stays ticked, and clicking the ticked one
/// clears it.
pub fn sync_theme(app: &AppHandle<Wry>, theme: Theme) {
    let Some(menu) = app.try_state::<ThemeMenu>() else { return };
    for (item, wanted) in [(&menu.dark, Theme::Dark), (&menu.light, Theme::Light), (&menu.system, Theme::System)] {
        if let Err(e) = item.set_checked(theme == wanted) {
            tracing::warn!("could not update the theme menu: {e}");
        }
    }
}

fn theme_for_id(id: &str) -> Option<Theme> {
    match id {
        "view.theme.dark" => Some(Theme::Dark),
        "view.theme.light" => Some(Theme::Light),
        "view.theme.system" => Some(Theme::System),
        _ => None,
    }
}

/// A menu item with a keyboard shortcut written the Windows way (`Ctrl+O`). On Windows and Linux
/// the shortcut is only shown, after a tab: the frontend handles the keys, with Ctrl or Cmd. On
/// macOS it becomes the item's real key equivalent with Cmd, which is how a Mac menu shows it;
/// a key the page has already handled never reaches the menu, so nothing runs twice.
fn item(app: &AppHandle<Wry>, id: &str, label: &str, shortcut: &str) -> tauri::Result<MenuItem<Wry>> {
    if cfg!(target_os = "macos") {
        MenuItemBuilder::with_id(id, label).accelerator(shortcut.replace("Ctrl+", "Cmd+")).build(app)
    } else {
        MenuItemBuilder::with_id(id, format!("{label}\t{shortcut}")).build(app)
    }
}

pub fn install(app: &AppHandle<Wry>) -> tauri::Result<()> {
    let mac = cfg!(target_os = "macos");
    let settings = item(app, "file.settings", "&Settings…", "Ctrl+,")?;
    // Not the predefined quit item, which exits without asking the window: closing the window
    // goes through the close guard in `shutdown`, which waits for Git to finish.
    let exit = if mac { item(app, "file.exit", "Quit StashPanda", "Ctrl+Q")? } else { MenuItemBuilder::with_id("file.exit", "E&xit").build(app)? };
    let mut file = SubmenuBuilder::new(app, "&File")
        .item(&item(app, "file.open", "&Open Repository…", "Ctrl+O")?)
        .text("file.clone", "C&lone Repository…")
        .item(&item(app, "file.close_tab", "&Close Tab", "Ctrl+W")?);
    // On a Mac, Settings and Quit live in the application menu instead.
    if !mac {
        file = file.separator().item(&settings).separator().item(&exit);
    }
    let file = file.build()?;

    let edit = SubmenuBuilder::new(app, "&Edit")
        .item(&PredefinedMenuItem::undo(app, None)?)
        .item(&PredefinedMenuItem::redo(app, None)?)
        .separator()
        .item(&PredefinedMenuItem::cut(app, None)?)
        .item(&PredefinedMenuItem::copy(app, None)?)
        .item(&PredefinedMenuItem::paste(app, None)?)
        .item(&PredefinedMenuItem::select_all(app, None)?)
        .separator()
        .item(&item(app, "edit.find", "&Find Commit…", "Ctrl+F")?)
        .build()?;

    let saved = app.try_state::<AppState>().map(|s| s.settings().appearance.theme).unwrap_or(Theme::Dark);
    let theme_dark = CheckMenuItemBuilder::with_id("view.theme.dark", "&Dark").checked(saved == Theme::Dark).build(app)?;
    let theme_light = CheckMenuItemBuilder::with_id("view.theme.light", "&Light").checked(saved == Theme::Light).build(app)?;
    let theme_system = CheckMenuItemBuilder::with_id("view.theme.system", "&System").checked(saved == Theme::System).build(app)?;
    let theme = SubmenuBuilder::new(app, "&Theme").items(&[&theme_dark, &theme_light, &theme_system]).build()?;
    let view = SubmenuBuilder::new(app, "&View")
        .item(&item(app, "view.refresh", "&Refresh", "F5")?)
        .separator()
        .item(&item(app, "view.toggle_console", "Toggle &Console", "Ctrl+`")?)
        .item(&item(app, "view.toggle_sidebar", "Toggle &Sidebar", "Ctrl+B")?)
        .separator()
        .item(&theme)
        .separator()
        .item(&PredefinedMenuItem::fullscreen(app, None)?)
        .build()?;

    // Edit, Rename and Remove pick the remote in the dialog, so the menu never needs rebuilding
    // when the active tab or its remotes change.
    let remotes = SubmenuBuilder::new(app, "Rem&otes")
        .text("repo.remote_add", "&Add Remote…")
        .text("repo.remote_edit", "&Edit Remote URL…")
        .text("repo.remote_rename", "Re&name Remote…")
        .text("repo.remote_remove", "&Remove Remote…")
        .build()?;
    let repository = SubmenuBuilder::new(app, "&Repository")
        // F5 belongs to View > Refresh; showing it twice would give a Mac two items on one key.
        .text("repo.refresh", if mac { "Re&fresh Repository" } else { "Re&fresh Repository\tF5" })
        .separator()
        .text("repo.create_branch", "Create &Branch…")
        .text("repo.create_tag", "Create &Tag…")
        .separator()
        .text("repo.stash", "&Stash Changes…")
        .text("repo.apply_stash", "&Apply Stash…")
        .separator()
        .text("repo.fetch", "&Fetch")
        .text("repo.pull", "&Pull")
        .text("repo.push", "Pus&h")
        .text("repo.sync", "S&ync (Pull, then Push)")
        .item(&remotes)
        .text("repo.settings", "Repository Settin&gs…")
        .separator()
        .text("repo.merge", "&Merge into the Current Branch…")
        .text("repo.rebase", "R&ebase the Current Branch…")
        .separator()
        .text("repo.remove", "&Remove from Library")
        .build()?;

    let mut help = SubmenuBuilder::new(app, "&Help").text("help.docs", "&Documentation").text("help.console_log", "Show &Console Log");
    if !mac {
        help = help.separator().text("help.about", "&About StashPanda");
    }
    let help = help.build()?;

    let menu = if mac {
        // A Mac's first menu is the application menu, named after the app whatever it is given.
        let app_menu = SubmenuBuilder::new(app, "StashPanda")
            .text("help.about", "About StashPanda")
            .separator()
            .item(&settings)
            .separator()
            .item(&PredefinedMenuItem::services(app, None)?)
            .separator()
            .item(&PredefinedMenuItem::hide(app, None)?)
            .item(&PredefinedMenuItem::hide_others(app, None)?)
            .item(&PredefinedMenuItem::show_all(app, None)?)
            .separator()
            .item(&exit)
            .build()?;
        let window = SubmenuBuilder::new(app, "Window")
            .item(&PredefinedMenuItem::minimize(app, None)?)
            .item(&PredefinedMenuItem::maximize(app, None)?)
            .build()?;
        MenuBuilder::new(app).items(&[&app_menu, &file, &edit, &view, &repository, &window, &help]).build()?
    } else {
        MenuBuilder::new(app).items(&[&file, &edit, &view, &repository, &help]).build()?
    };
    // Debug builds get a Developer menu: WebView2's own shortcut for the inspector is off (see
    // `webview::disable_browser_shortcuts`), and the other platforms never had one.
    #[cfg(debug_assertions)]
    menu.append(&SubmenuBuilder::new(app, "&Developer").text("dev.inspector", "Open Dev &Inspector").build()?)?;
    app.set_menu(menu)?;
    app.manage(ThemeMenu { dark: theme_dark, light: theme_light, system: theme_system });
    app.on_menu_event(|app, event| {
        let id = event.id().0.clone();
        #[cfg(debug_assertions)]
        if id == "dev.inspector" {
            crate::webview::open_inspector(app);
            return;
        }
        // Put the ticks right at once; the frontend then saves the setting, and
        // settings_update syncs them again.
        if let Some(theme) = theme_for_id(&id) {
            sync_theme(app, theme);
        }
        if id == "file.exit" {
            for window in app.webview_windows().into_values() {
                let _ = window.close();
            }
            return;
        }
        let _ = MenuActionEvent { id }.emit(app);
    });
    Ok(())
}
