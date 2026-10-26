//! StashPanda: a standalone desktop Git GUI that runs your own Git.
//!
//! Module map (see docs/architecture.md):
//! - `error`      the single error enum every command returns
//! - `settings`   settings.json model, load/save, merge patch
//! - `ui_state`   remembered UI state model (stored in the database)
//! - `persist`    debounced writer for settings.json and the UI state rows
//! - `storage`    SQLite database: commit and ref cache, remembered UI state
//! - `state`      AppState managed by Tauri
//! - `ipc`        Tauri commands and typed events

pub mod error;
pub mod git;
pub mod graph;
pub mod ipc;
pub mod keys;
pub mod menu;
pub mod platform;
pub mod repo;
pub mod persist;
pub mod settings;
pub mod shutdown;
pub mod state;
pub mod storage;
pub mod transport;
pub mod ui_state;
pub mod watch;
pub mod webview;
#[cfg(test)]
mod keys_probe_test;

use tauri::Manager;
use tauri_specta::{collect_commands, collect_events, Builder};

fn specta_builder() -> Builder<tauri::Wry> {
    Builder::<tauri::Wry>::new()
        .commands(collect_commands![
            ipc::settings::settings_get,
            ipc::settings::settings_update,
            ipc::settings::ui_state_get,
            ipc::settings::ui_state_update,
            ipc::settings::startup_warnings,
            ipc::fonts::fonts_monospace,
            ipc::repos::repo_resolve,
            ipc::repos::clone_inspect,
            ipc::repos::repo_clone,
            ipc::repos::clone_cancel,
            ipc::repos::repo_add,
            ipc::repos::repo_update,
            ipc::repos::repo_remove,
            ipc::repos::repo_open,
            ipc::repos::repo_fix_safe_directory,
            ipc::repos::repo_close,
            ipc::repos::repo_list_open,
            ipc::repos::repo_heads,
            ipc::repos::wsl_list_distros,
            ipc::repos::host_info,
            ipc::repos::ssh_trust_host,
            ipc::repos::ssh_set_password,
            ipc::query::get_repo_status,
            ipc::query::get_host_path,
            ipc::query::set_repo_activity,
            ipc::query::get_pending_message,
            ipc::query::get_repo_refs,
            ipc::query::get_commit_changes,
            ipc::query::get_commit_message,
            ipc::query::get_merge_info,
            ipc::query::get_file_diff,
            ipc::query::get_modified_times,
            ipc::graph::graph_set_hidden_refs,
            ipc::graph::graph_set_ref_colours,
            ipc::repos::repo_set_colour,
            ipc::query::get_blob_content,
            ipc::query::get_worktree_file_content,
            ipc::query::get_conflict_file,
            ipc::query::get_file_history,
            ipc::query::get_file_blame,
            ipc::query::get_tag_annotation,
            ipc::query::get_git_config,
            ipc::query::get_repo_stashes,
            ipc::graph::graph_open,
            ipc::graph::graph_meta,
            ipc::graph::graph_rows,
            ipc::graph::graph_find,
            ipc::graph::graph_search,
            ipc::graph::graph_refresh,
            ipc::keys::keys_default_dirs,
            ipc::keys::keys_scan,
            ipc::keys::keys_inspect,
            ipc::keys::keys_unlock,
            ipc::keys::keys_forget_passphrase,
            ipc::keys::keys_export_public,
            ipc::keys::keys_convert,
            ipc::keys::keys_generate,
            ipc::keys::keys_change_passphrase,
            ipc::keys::keys_import,
            ipc::keys::keys_agent_list,
            ipc::keys::keys_agent_add,
            ipc::keys::keys_keychain_status,
            ipc::keys::keys_delete,
            ipc::keys::keys_read_file,
            ipc::keys::keys_default_comment,
            ipc::ops::op_checkout,
            ipc::ops::op_branch_create,
            ipc::ops::op_branch_delete,
            ipc::ops::op_branch_rename,
            ipc::ops::get_fast_forwards_to_head,
            ipc::ops::op_branch_fast_forward,
            ipc::ops::op_branch_fast_forward_to_upstream,
            ipc::ops::op_push_to,
            ipc::ops::get_commits_dropped,
            ipc::ops::op_branch_set_upstream,
            ipc::ops::op_reset,
            ipc::ops::op_cherry_pick,
            ipc::ops::op_cherry_pick_abort,
            ipc::ops::op_revert,
            ipc::ops::op_revert_abort,
            ipc::ops::op_merge,
            ipc::ops::op_merge_abort,
            ipc::ops::op_tag_create,
            ipc::ops::op_tag_delete,
            ipc::ops::op_stash_push,
            ipc::ops::op_stash_apply,
            ipc::ops::op_stash_pop,
            ipc::ops::op_stash_drop,
            ipc::ops::op_stage,
            ipc::ops::op_unstage,
            ipc::ops::op_discard,
            ipc::ops::op_apply_lines,
            ipc::ops::op_commit,
            ipc::ops::op_remove_stale_lock,
            ipc::ops::op_rebase_abort,
            ipc::ops::op_rebase,
            ipc::ops::op_continue,
            ipc::ops::op_skip,
            ipc::ops::op_resolve_conflicts,
            ipc::ops::op_restore_conflict,
            ipc::ops::write_worktree_file,
            ipc::ops::op_pull,
            ipc::ops::op_cancel,
            ipc::ops::op_fetch,
            ipc::ops::op_push,
            ipc::ops::op_delete_remote_branch,
            ipc::ops::op_push_tags,
            ipc::ops::op_remote_add,
            ipc::ops::op_remote_set_url,
            ipc::ops::op_remote_rename,
            ipc::ops::op_remote_remove,
            ipc::ops::op_config_change,
            ipc::ops::op_sync,
            ipc::ops::get_upstream,
            ipc::ops::get_push_target,
            ipc::ops::get_head_on_push_target,
            ipc::ops::get_rewrote_pushed_commits,
            ipc::ops::op_set_push_default,
            ipc::ops::get_pull_mode,
        ])
        .events(collect_events![
            ipc::events::SettingsChanged,
            transport::ssh_pool::TransportState,
            graph::loader::GraphLoadFailedEvent,
            ipc::events::UiStateChanged,
            ipc::events::AppWarning,
            ipc::events::OperationProgress,
            ipc::events::CloneProgress,
            git::console::ConsoleEvent,
            graph::loader::GraphSnapshotEvent,
            graph::loader::GraphProgressEvent,
            watch::RepoChangedEvent,
            watch::WatcherStateEvent,
            menu::MenuActionEvent,
        ])
        // u64 timestamps and sizes fit comfortably in a JS number.
        .dangerously_cast_bigints_to_number()
        .disable_serde_phases()
}

fn init_tracing() {
    use tracing_subscriber::EnvFilter;
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("stashpanda_lib=debug,info"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .try_init();
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Before any other thread starts: setting an environment variable is not thread safe.
    #[cfg(target_os = "macos")]
    platform::adopt_login_shell_path();
    init_tracing();

    let builder = specta_builder();

    #[cfg(debug_assertions)]
    builder
        .export(
            specta_typescript::Typescript::default().header("// @ts-nocheck\n// Generated by tauri-specta from src-tauri. Do not edit by hand.\n"),
            "../src/lib/bindings.ts",
        )
        .expect("failed to export TypeScript bindings");

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_window_state::Builder::default().build())
        .invoke_handler(builder.invoke_handler())
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if shutdown::defer_close(window.app_handle(), window) {
                    api.prevent_close();
                }
            }
        })
        .setup(move |app| {
            builder.mount_events(app);
            let state = state::AppState::init(app.handle())?;
            app.manage(state);
            menu::install(app.handle())?;
            webview::disable_browser_shortcuts(app.handle());
            #[cfg(debug_assertions)]
            webview::clear_dev_browsing_data(app.handle());
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| {
            if let tauri::RunEvent::ExitRequested { api, .. } = event {
                // Quitting without closing the window first (macOS: the Dock, logging out) waits
                // for Git the same way closing the window does.
                if shutdown::defer_exit(app) {
                    api.prevent_exit();
                    return;
                }
                app.state::<state::AppState>().flush();
            }
        });
}

#[cfg(test)]
mod tests {
    /// Regenerates `src/lib/bindings.ts` from the command and event list.
    /// Run with `cargo test export_bindings` after changing any IPC type.
    #[test]
    fn export_bindings() {
        super::specta_builder()
            .export(
                specta_typescript::Typescript::default().header(
                    "// @ts-nocheck\n// Generated by tauri-specta from src-tauri. Do not edit by hand.\n",
                ),
                concat!(env!("CARGO_MANIFEST_DIR"), "/../src/lib/bindings.ts"),
            )
            .expect("export bindings");
    }
}
