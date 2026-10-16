//! Closing the window while Git is changing a repository.
//!
//! Every local write (a pull's merge or rebase, a checkout, a commit) holds its repository's
//! exclusive lock for as long as Git runs, and quitting kills the Git it started. A rebase killed
//! part way leaves the repository mid-rebase with a commit half applied. So the first close
//! request while any repository's lock is held waits: the window stays open, a warning says so,
//! and it closes by itself once those writes have finished. A second close request quits at once,
//! so a hook that never returns cannot keep the window open.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use tauri::{AppHandle, Manager, Window};
use tauri_specta::Event;

use crate::ipc::events::AppWarning;
use crate::repo::RepoHandle;
use crate::state::AppState;

/// Set while a close is waiting for writes to finish.
static WAITING: AtomicBool = AtomicBool::new(false);

/// Repositories whose tab was closed, or that were removed from the library, while Git may still
/// be writing to them: they are no longer in `AppState::repos`, but closing the window must still
/// wait for them. Each leaves the list once nothing holds its lock.
static RETIRED: std::sync::Mutex<Vec<Arc<RepoHandle>>> = std::sync::Mutex::new(Vec::new());

/// Called with a repository taken out of `AppState::repos`, so a close waits for its writes too.
pub fn retire(handle: Arc<RepoHandle>) {
    let mut retired = RETIRED.lock().expect("retired repositories lock");
    retired.retain(|h| h.lock.try_read().is_err());
    retired.push(handle);
}

/// Called on a close request for `window`: true when the close should be prevented now, because
/// a repository is being written to and this is the first request. It then closes `window` once
/// those writes are done.
pub fn defer_close(app: &AppHandle, window: &Window) -> bool {
    let busy = busy_repos(app);
    if busy.is_empty() || WAITING.swap(true, Ordering::SeqCst) {
        return false;
    }
    let _ = AppWarning {
        message: "StashPanda will close as soon as Git has finished changing the repository, so that it is not left half done. Close the window again to quit at once.".into(),
    }
    .emit(app);
    let window = window.clone();
    tauri::async_runtime::spawn(async move {
        for h in busy {
            drop(h.lock.read().await);
        }
        // Asks again, so a write that started meanwhile is waited for too.
        WAITING.store(false, Ordering::SeqCst);
        let _ = window.close();
    });
    true
}

/// The repositories, open or just closed (`retire`), whose exclusive lock is held, or wanted, by
/// a write.
fn busy_repos(app: &AppHandle) -> Vec<Arc<RepoHandle>> {
    let mut busy: Vec<Arc<RepoHandle>> = {
        let mut retired = RETIRED.lock().expect("retired repositories lock");
        retired.retain(|h| h.lock.try_read().is_err());
        retired.clone()
    };
    let state = app.state::<AppState>();
    if let Ok(repos) = state.repos.try_read() {
        busy.extend(repos.values().filter(|h| h.lock.try_read().is_err()).cloned());
    }
    busy
}
