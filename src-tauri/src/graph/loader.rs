//! Builds and swaps a repository's graph snapshot; emits `graph_snapshot`.

use std::sync::atomic::Ordering;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::AppHandle;
use tauri_specta::Event;

use super::snapshot::{GraphSnapshot, WorktreeSummary};
use super::store::CommitStore;
use crate::error::Result;
use crate::git::queries::{log, refs, status};
use crate::repo::RepoHandle;
use crate::settings::{CacheSettings, GraphOrder};
use crate::storage::repo_cache;
use crate::storage::Db;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum SnapshotReason {
    Cache,
    Live,
    Refresh,
    Status,
    /// Refs were switched on or off in the graph.
    Filter,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
#[serde(rename_all = "camelCase")]
pub struct GraphSnapshotEvent {
    pub repo_id: String,
    pub version: u64,
    pub total: u32,
    pub complete: bool,
    pub truncated: bool,
    pub max_lanes: u16,
    pub open_lanes: u32,
    pub head: Option<String>,
    pub head_ref: Option<String>,
    pub worktree: Option<WorktreeSummary>,
    pub reason: SnapshotReason,
}

impl GraphSnapshotEvent {
    pub fn from_snapshot(repo_id: &str, snap: &GraphSnapshot, reason: SnapshotReason) -> GraphSnapshotEvent {
        let m = snap.meta();
        GraphSnapshotEvent {
            repo_id: repo_id.to_string(),
            version: m.version,
            total: m.total,
            complete: m.complete,
            truncated: m.truncated,
            max_lanes: m.max_lanes,
            open_lanes: m.open_lanes,
            head: m.head,
            head_ref: m.head_ref,
            worktree: m.worktree,
            reason,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
#[serde(rename_all = "camelCase")]
pub struct GraphProgressEvent {
    pub repo_id: String,
    pub loaded: u32,
}

pub fn worktree_summary(s: &status::StatusSnapshot) -> Option<WorktreeSummary> {
    if !s.is_dirty() {
        return None;
    }
    Some(WorktreeSummary {
        staged: s.staged_count,
        unstaged: s.unstaged_count,
        untracked: s.untracked_count,
        conflicted: s.conflicted_count,
    })
}

/// Full live load: refs, status and log, then a fresh snapshot.
pub async fn refresh_full(
    app: Option<&AppHandle>,
    db: Option<&Arc<Db>>,
    handle: &Arc<RepoHandle>,
    order: GraphOrder,
    cache: &CacheSettings,
    reason: SnapshotReason,
) -> Result<Arc<GraphSnapshot>> {
    let client = &handle.client;
    let _shared = handle.lock.read().await;

    let (refs_res, status_res) = tokio::join!(refs::get_refs(client), status::get_status(client));
    rebuild(app, db, handle, order, cache, reason, refs_res?, status_res?).await
}

/// What a change-driven refresh found different from the current snapshot.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Changes {
    pub status: bool,
    pub refs: bool,
}

impl Changes {
    pub fn any(self) -> bool {
        self.status || self.refs
    }
}

/// Refresh after something may have changed (a watcher burst, or a write
/// operation that just finished): read refs and status, and rebuild only what
/// differs from the current snapshot. When both match, nothing is rebuilt,
/// emitted or persisted, so the watcher's follow-up to the app's own write
/// costs one `for-each-ref` and one `status` instead of a second `git log`.
/// With `check_refs` false only status is read; a moved HEAD still rebuilds.
pub async fn refresh_if_changed(
    app: Option<&AppHandle>,
    db: Option<&Arc<Db>>,
    handle: &Arc<RepoHandle>,
    order: GraphOrder,
    cache: &CacheSettings,
    check_refs: bool,
) -> Result<Changes> {
    let client = &handle.client;
    let _shared = handle.lock.read().await;

    let (refs_snap, status_snap) = if check_refs {
        let (r, s) = tokio::join!(refs::get_refs(client), status::get_status(client));
        (Some(r?), s?)
    } else {
        (None, status::get_status(client).await?)
    };
    let status_changed = handle.last_status.read().await.as_ref() != Some(&status_snap);

    // `git log` walks exactly the refs in `RefsSnapshot` plus HEAD, so equal
    // refs mean the commit set is unchanged.
    let current = handle.graph.load_full().filter(|c| c.complete);
    let refs_changed = match &current {
        None => true,
        Some(c) => {
            status_snap.head_oid.as_deref() != c.refs.head.as_deref()
                || refs_snap.as_ref().is_some_and(|r| r != c.refs.as_ref())
        }
    };
    if refs_changed {
        let refs_snap = match refs_snap {
            Some(r) => r,
            None => refs::get_refs(client).await?,
        };
        rebuild(app, db, handle, order, cache, SnapshotReason::Refresh, refs_snap, status_snap).await?;
        return Ok(Changes { status: status_changed, refs: true });
    }
    if status_changed {
        if let Some(c) = &current {
            apply_status(app, handle, c, status_snap, order).await;
        }
    }
    Ok(Changes { status: status_changed, refs: false })
}

/// Log, build, swap, emit and persist. The caller holds the shared lock.
#[allow(clippy::too_many_arguments)]
async fn rebuild(
    app: Option<&AppHandle>,
    db: Option<&Arc<Db>>,
    handle: &Arc<RepoHandle>,
    order: GraphOrder,
    cache: &CacheSettings,
    reason: SnapshotReason,
    refs_snap: refs::RefsSnapshot,
    status_snap: status::StatusSnapshot,
) -> Result<Arc<GraphSnapshot>> {
    let client = &handle.client;
    let max = cache.max_commits_per_repo.max(1);
    // An unborn branch has no HEAD commit; `git log ... HEAD` would fail.
    let records = if refs_snap.head.is_none() && refs_snap.refs.is_empty() {
        Vec::new()
    } else {
        log::get_log(client, max).await?
    };
    let truncated = records.len() as u32 >= max;
    let mut store = CommitStore::new();
    for r in &records {
        store.insert(r);
    }

    let version = handle.graph_version.fetch_add(1, Ordering::Relaxed) + 1;
    let snap = Arc::new(GraphSnapshot::build(
        Arc::new(store),
        Arc::new(refs_snap),
        worktree_summary(&status_snap),
        order,
        version,
        true,
        truncated,
        &handle.hidden_refs(),
    ));
    handle.graph.store(Some(snap.clone()));
    *handle.last_status.write().await = Some(status_snap);

    if let Some(app) = app {
        let _ = GraphSnapshotEvent::from_snapshot(&handle.key, &snap, reason).emit(app);
    }
    if let Some(db) = db {
        let key = handle.key.clone();
        let to_save = snap.clone();
        let started = std::time::Instant::now();
        if let Err(e) = db.run(move |c| repo_cache::persist(c, &key, &to_save)).await {
            tracing::warn!("could not persist graph cache for {}: {e}", handle.key);
        } else {
            tracing::debug!("persisted {} commits for {} in {:?}", snap.store.len(), handle.key, started.elapsed());
        }
    }
    Ok(snap)
}

/// Paint from the cache if there is one. Returns None when nothing is cached.
pub async fn load_from_cache(
    app: Option<&AppHandle>,
    db: &Arc<Db>,
    handle: &Arc<RepoHandle>,
    order: GraphOrder,
) -> Result<Option<Arc<GraphSnapshot>>> {
    let key = handle.key.clone();
    let started = std::time::Instant::now();
    let Some(cached) = db.run(move |c| repo_cache::load(c, &key)).await? else {
        return Ok(None);
    };
    let version = handle.graph_version.fetch_add(1, Ordering::Relaxed) + 1;
    let snap = Arc::new(GraphSnapshot::build(
        Arc::new(cached.store),
        Arc::new(cached.refs),
        None,
        order,
        version,
        true,
        cached.truncated,
        &handle.hidden_refs(),
    ));
    tracing::debug!("cache paint for {} with {} commits took {:?}", handle.key, snap.store.len(), started.elapsed());
    // Do not overwrite a live snapshot that landed first.
    if handle.graph.load_full().is_some() {
        return Ok(None);
    }
    handle.graph.store(Some(snap.clone()));
    if let Some(app) = app {
        let _ = GraphSnapshotEvent::from_snapshot(&handle.key, &snap, SnapshotReason::Cache).emit(app);
    }
    Ok(Some(snap))
}

/// The open sequence: cache paint (if any), then a live load that persists.
pub async fn open_pipeline(
    app: &AppHandle,
    db: &Arc<Db>,
    handle: &Arc<RepoHandle>,
    order: GraphOrder,
    cache: &CacheSettings,
) {
    if let Err(e) = load_from_cache(Some(app), db, handle, order).await {
        tracing::warn!("cache load for {} failed: {e}", handle.key);
    }
    if let Err(e) = refresh_full(Some(app), Some(db), handle, order, cache, SnapshotReason::Live).await {
        tracing::warn!("live graph load for {} failed: {e}", handle.key);
    }
}

/// Status changed but refs did not: rebuild with or without the Working Tree
/// row, reusing the existing store and refs. Falls back to a full refresh
/// when there is no snapshot yet.
pub async fn refresh_status_only(
    app: Option<&AppHandle>,
    handle: &Arc<RepoHandle>,
    order: GraphOrder,
    cache: &CacheSettings,
) -> Result<Arc<GraphSnapshot>> {
    let Some(current) = handle.graph.load_full() else {
        return refresh_full(app, None, handle, order, cache, SnapshotReason::Status).await;
    };
    let status_snap = {
        let _shared = handle.lock.read().await;
        status::get_status(&handle.client).await?
    };
    if status_snap.head_oid.as_deref() != current.refs.head.as_deref() {
        // HEAD moved: refs are stale too.
        return refresh_full(app, None, handle, order, cache, SnapshotReason::Refresh).await;
    }
    Ok(apply_status(app, handle, &current, status_snap, order).await)
}

/// Rebuild the current snapshot with the handle's hidden refs, from the commits already loaded:
/// no Git command runs and nothing is persisted. None when there is no snapshot yet; the first
/// load then uses the hidden refs by itself.
pub fn rebuild_filtered(app: Option<&AppHandle>, handle: &Arc<RepoHandle>, order: GraphOrder) -> Option<Arc<GraphSnapshot>> {
    let current = handle.graph.load_full()?;
    let version = handle.graph_version.fetch_add(1, Ordering::Relaxed) + 1;
    let snap = Arc::new(GraphSnapshot::build(
        current.store.clone(),
        current.refs.clone(),
        current.worktree,
        order,
        version,
        current.complete,
        current.truncated,
        &handle.hidden_refs(),
    ));
    handle.graph.store(Some(snap.clone()));
    if let Some(app) = app {
        let _ = GraphSnapshotEvent::from_snapshot(&handle.key, &snap, SnapshotReason::Filter).emit(app);
    }
    Some(snap)
}

/// Store the new status, and swap in a snapshot with or without the Working
/// Tree row when its summary changed.
async fn apply_status(
    app: Option<&AppHandle>,
    handle: &Arc<RepoHandle>,
    current: &Arc<GraphSnapshot>,
    status_snap: status::StatusSnapshot,
    order: GraphOrder,
) -> Arc<GraphSnapshot> {
    let wt = worktree_summary(&status_snap);
    if wt == current.worktree {
        *handle.last_status.write().await = Some(status_snap);
        return current.clone();
    }
    let version = handle.graph_version.fetch_add(1, Ordering::Relaxed) + 1;
    let snap = Arc::new(GraphSnapshot::build(
        current.store.clone(),
        current.refs.clone(),
        wt,
        order,
        version,
        current.complete,
        current.truncated,
        &handle.hidden_refs(),
    ));
    handle.graph.store(Some(snap.clone()));
    *handle.last_status.write().await = Some(status_snap);
    if let Some(app) = app {
        let _ = GraphSnapshotEvent::from_snapshot(&handle.key, &snap, SnapshotReason::Status).emit(app);
    }
    snap
}

#[cfg(test)]
mod e2e {
    use std::path::PathBuf;
    use std::process::Command;

    use super::*;
    use crate::platform::Host;
    use crate::repo::open::open_repo;
    use crate::settings::{RepoEntry, Settings};
    use crate::storage::Db;

    fn git_at(dir: &PathBuf, when: i64, args: &[&str]) {
        let stamp = format!("{when} +0000");
        let st = Command::new("git")
            .arg("-C").arg(dir)
            .env("GIT_COMMITTER_DATE", &stamp)
            .env("GIT_AUTHOR_DATE", &stamp)
            .args(["-c", "user.email=t@t", "-c", "user.name=T", "-c", "commit.gpgsign=false"])
            .args(args)
            .status()
            .expect("git runs");
        assert!(st.success(), "git {:?} failed", args);
    }
    fn git(dir: &PathBuf, args: &[&str]) {
        git_at(dir, 1_700_000_000, args)
    }

    #[tokio::test]
    async fn open_build_persist_and_reload_from_cache() {
        if which::which("git").is_err() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("stashpanda-graph-e2e-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        git(&dir, &["init", "-q", "-b", "main"]);
        for i in 0..5 {
            std::fs::write(dir.join("f.txt"), format!("{i}\n")).unwrap();
            git(&dir, &["add", "-A"]);
            git_at(&dir, 1_700_000_000 + i * 60, &["commit", "-qm", &format!("c{i}")]);
        }
        git(&dir, &["checkout", "-qb", "topic", "HEAD~2"]);
        std::fs::write(dir.join("t.txt"), "t\n").unwrap();
        git(&dir, &["add", "-A"]);
        // Older than the main tip, so date order puts main's tip first.
        git_at(&dir, 1_700_000_000 + 3 * 60 + 30, &["commit", "-qm", "topic"]);
        git(&dir, &["checkout", "-q", "main"]);
        std::fs::write(dir.join("dirty.txt"), "x\n").unwrap();

        let settings = Settings::default();
        let entry = RepoEntry { key: "e2e".into(), path: dir.to_string_lossy().into_owned(), ..Default::default() };
        let host = Host::detect();
        let handle = open_repo(None, &entry, &settings, &host, None).await.expect("open");
        assert!(!handle.info.is_bare);
        assert!(handle.info.toplevel.ends_with(dir.file_name().unwrap().to_str().unwrap()));

        let db = Db::open_in_memory().unwrap();
        let snap = refresh_full(None, Some(&db), &handle, GraphOrder::Date, &settings.cache, SnapshotReason::Live).await.expect("refresh");
        // 6 commits + the Working Tree row for the untracked file.
        assert_eq!(snap.total(), 7);
        let rows = snap.rows(0, 10);
        assert_eq!(rows[0].kind, crate::graph::snapshot::RowKind::WorkingTree);
        assert_eq!(rows[0].worktree.unwrap().untracked, 1);
        assert!(rows[1].is_head, "HEAD (main tip) is the first commit row");
        assert_eq!(rows[1].lane, 0);
        assert!(rows.iter().any(|r| r.refs.iter().any(|l| l.short == "topic")));
        let topic_row = rows.iter().find(|r| r.subject == "topic").unwrap();
        assert_eq!(topic_row.lane, 1, "the side branch sits in lane 1");
        assert_eq!(snap.max_lanes, 2);

        // Cache round trip: a fresh handle paints from the cache with the same rows.
        let handle2 = open_repo(None, &entry, &settings, &host, None).await.expect("open again");
        let cached = load_from_cache(None, &db, &handle2, GraphOrder::Date).await.expect("cache").expect("present");
        let live: Vec<String> = snap.rows(1, 10).into_iter().map(|r| r.oid).collect();
        let from_cache: Vec<String> = cached.rows(0, 10).into_iter().map(|r| r.oid).collect();
        assert_eq!(live, from_cache, "cache order equals live order (minus the working tree row)");

        // Status-only refresh after cleaning the tree drops the Working Tree row.
        std::fs::remove_file(dir.join("dirty.txt")).unwrap();
        let snap2 = refresh_status_only(None, &handle, GraphOrder::Date, &settings.cache).await.expect("status refresh");
        assert_eq!(snap2.total(), 6);
        assert!(snap2.version > snap.version);

        // Nothing changed since the last refresh: no rebuild, same version.
        let none = refresh_if_changed(None, Some(&db), &handle, GraphOrder::Date, &settings.cache, true).await.expect("no-op refresh");
        assert_eq!(none, Changes::default());
        assert_eq!(handle.graph.load_full().unwrap().version, snap2.version);

        // A new untracked file: status changed, refs did not.
        std::fs::write(dir.join("new.txt"), "n\n").unwrap();
        let st = refresh_if_changed(None, Some(&db), &handle, GraphOrder::Date, &settings.cache, true).await.expect("status refresh");
        assert_eq!(st, Changes { status: true, refs: false });
        let snap3 = handle.graph.load_full().unwrap();
        assert_eq!(snap3.total(), 7, "Working Tree row is back");

        // A commit moves HEAD: full rebuild, even when only status was asked for.
        git(&dir, &["add", "-A"]);
        git_at(&dir, 1_700_001_000, &["commit", "-qm", "c5"]);
        let full = refresh_if_changed(None, Some(&db), &handle, GraphOrder::Date, &settings.cache, false).await.expect("full refresh");
        assert!(full.refs && full.status, "{full:?}");
        let snap4 = handle.graph.load_full().unwrap();
        assert_eq!(snap4.total(), 7, "7 commits, clean tree");
        assert!(snap4.version > snap3.version);

        handle.close().await;
        handle2.close().await;
        let _ = std::fs::remove_dir_all(&dir);
    }
}
