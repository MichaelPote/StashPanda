//! The native application menu bar. Clicks are forwarded to the frontend as
//! `menu_action` events carrying the item id; the frontend owns the behaviour
//! so it can use its dialogs, stores and the active tab.

use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::menu::{CheckMenuItem, CheckMenuItemBuilder, MenuBuilder, PredefinedMenuItem, SubmenuBuilder};
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

pub fn install(app: &AppHandle<Wry>) -> tauri::Result<()> {
    let file = SubmenuBuilder::new(app, "&File")
        .text("file.open", "&Open Repository…\tCtrl+O")
        .text("file.close_tab", "&Close Tab\tCtrl+W")
        .separator()
        .text("file.settings", "&Settings…\tCtrl+,")
        .separator()
        .item(&PredefinedMenuItem::quit(app, Some("E&xit"))?)
        .build()?;

    let edit = SubmenuBuilder::new(app, "&Edit")
        .item(&PredefinedMenuItem::undo(app, None)?)
        .item(&PredefinedMenuItem::redo(app, None)?)
        .separator()
        .item(&PredefinedMenuItem::cut(app, None)?)
        .item(&PredefinedMenuItem::copy(app, None)?)
        .item(&PredefinedMenuItem::paste(app, None)?)
        .item(&PredefinedMenuItem::select_all(app, None)?)
        .separator()
        .text("edit.find", "&Find Commit…\tCtrl+F")
        .build()?;

    let saved = app.try_state::<AppState>().map(|s| s.settings().appearance.theme).unwrap_or(Theme::Dark);
    let theme_dark = CheckMenuItemBuilder::with_id("view.theme.dark", "&Dark").checked(saved == Theme::Dark).build(app)?;
    let theme_light = CheckMenuItemBuilder::with_id("view.theme.light", "&Light").checked(saved == Theme::Light).build(app)?;
    let theme_system = CheckMenuItemBuilder::with_id("view.theme.system", "&System").checked(saved == Theme::System).build(app)?;
    let theme = SubmenuBuilder::new(app, "&Theme").items(&[&theme_dark, &theme_light, &theme_system]).build()?;
    let view = SubmenuBuilder::new(app, "&View")
        .text("view.refresh", "&Refresh\tF5")
        .separator()
        .text("view.toggle_console", "Toggle &Console\tCtrl+`")
        .text("view.toggle_sidebar", "Toggle &Sidebar\tCtrl+B")
        .separator()
        .item(&theme)
        .separator()
        .item(&PredefinedMenuItem::fullscreen(app, None)?)
        .build()?;

    let repository = SubmenuBuilder::new(app, "&Repository")
        .text("repo.refresh", "Re&fresh Repository\tF5")
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
        .separator()
        .text("repo.remove", "&Remove from Library")
        .build()?;

    let help = SubmenuBuilder::new(app, "&Help")
        .text("help.docs", "&Documentation")
        .text("help.console_log", "Show &Console Log")
        .separator()
        .text("help.about", "&About StashPanda")
        .build()?;

    let menu = MenuBuilder::new(app).items(&[&file, &edit, &view, &repository, &help]).build()?;
    app.set_menu(menu)?;
    app.manage(ThemeMenu { dark: theme_dark, light: theme_light, system: theme_system });
    app.on_menu_event(|app, event| {
        let id = event.id().0.clone();
        // Put the ticks right at once; the frontend then saves the setting, and
        // settings_update syncs them again.
        if let Some(theme) = theme_for_id(&id) {
            sync_theme(app, theme);
        }
        let _ = MenuActionEvent { id }.emit(app);
    });
    Ok(())
}
