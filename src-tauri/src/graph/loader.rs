//! Builds and swaps a repository's graph snapshot; emits `graph_snapshot`.

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

use rustc_hash::FxHashSet;
use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::AppHandle;
use tauri_specta::Event;

use super::oid::Oid;
use super::snapshot::{GraphSnapshot, WorktreeSummary};
use super::store::CommitStore;
use crate::error::Result;
use crate::git::queries::{log, refs, status};
use crate::repo::RepoHandle;
use crate::settings::{CacheSettings, GraphOptions};
use crate::storage::repo_cache;
use crate::storage::Db;

/// More new commits than this and an incremental refresh gives way to a full load.
const INCREMENTAL_LIMIT: u32 = 20_000;
/// Rows in the first paint from a large repository's cache.
const FIRST_PAGE_ROWS: u32 = 500;
/// A cache holding more commits than this paints its first page before it loads whole. The
/// whole load of 85,000 commits (git.git) took 257 ms in a release build and 1.2 s in a debug one.
const FIRST_PAGE_ABOVE: u32 = 10_000;
/// How often a live load with nothing complete on screen swaps in what has arrived so far.
const PARTIAL_EVERY: Duration = Duration::from_millis(500);
/// How often a live load reports its count in `graph_progress`.
const PROGRESS_EVERY: u32 = 2_000;

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

/// The Working Tree row's counts. Every repository with a working tree gets the row, clean or not
/// (all counts zero), so it can always be selected; a bare repository has none.
pub fn worktree_summary(handle: &RepoHandle, s: &status::StatusSnapshot) -> Option<WorktreeSummary> {
    if handle.info.is_bare {
        return None;
    }
    Some(WorktreeSummary {
        staged: s.staged_count,
        unstaged: s.unstaged_count,
        untracked: s.untracked_count,
        conflicted: s.conflicted_count,
    })
}

/// Live load: refs and status, then the commits, then a fresh snapshot. The commits come
/// incrementally from the current snapshot when there is a complete one (see `incremental_store`),
/// otherwise from a full `git log`; `force_full` always reads the whole history.
#[allow(clippy::too_many_arguments)]
pub async fn refresh_full(
    app: Option<&AppHandle>,
    db: Option<&Arc<Db>>,
    handle: &Arc<RepoHandle>,
    options: GraphOptions,
    cache: &CacheSettings,
    reason: SnapshotReason,
    force_full: bool,
) -> Result<Arc<GraphSnapshot>> {
    let client = &handle.client;
    let _shared = handle.lock.read().await;

    let (refs_res, status_res) = crate::git::queries::refs_and_status(client, &handle.info.git_dir).await;
    rebuild(app, db, handle, options, cache, reason, refs_res?, status_res?, force_full).await
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
    options: GraphOptions,
    cache: &CacheSettings,
    check_refs: bool,
) -> Result<Changes> {
    let client = &handle.client;
    let _shared = handle.lock.read().await;

    let (refs_snap, status_snap) = if check_refs {
        let (r, s) = crate::git::queries::refs_and_status(client, &handle.info.git_dir).await;
        (Some(r?), s?)
    } else {
        (None, crate::git::queries::full_status(client, &handle.info.git_dir).await?)
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
        rebuild(app, db, handle, options, cache, SnapshotReason::Refresh, refs_snap, status_snap, false).await?;
        return Ok(Changes { status: status_changed, refs: true });
    }
    if status_changed {
        if let Some(c) = &current {
            apply_status(app, handle, c, status_snap, options).await;
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
    options: GraphOptions,
    cache: &CacheSettings,
    reason: SnapshotReason,
    refs_snap: refs::RefsSnapshot,
    status_snap: status::StatusSnapshot,
    force_full: bool,
) -> Result<Arc<GraphSnapshot>> {
    let max = cache.max_commits_per_repo.max(1);
    let refs_snap = Arc::new(refs_snap);
    let worktree = worktree_summary(handle, &status_snap);
    // An unborn branch has no HEAD commit; `git log ... HEAD` would fail.
    let (store, truncated) = if refs_snap.head.is_none() && refs_snap.refs.is_empty() {
        (Arc::new(CommitStore::new()), false)
    } else {
        // A truncated store lacks the history below the cap, which an incremental log takes as known.
        let base = if force_full { None } else { handle.graph.load_full().filter(|s| s.complete && !s.truncated) };
        let incremental = match &base {
            Some(base) => incremental_store(handle, base, &refs_snap, INCREMENTAL_LIMIT, max).await,
            None => None,
        };
        match incremental {
            Some(store) => (store, false),
            None => {
                let (store, truncated) = stream_full(app, handle, options, max, &refs_snap, worktree).await?;
                (Arc::new(store), truncated)
            }
        }
    };

    let version = handle.graph_version.fetch_add(1, Ordering::Relaxed) + 1;
    let snap = Arc::new(GraphSnapshot::build(
        store,
        refs_snap,
        worktree,
        options,
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

/// The branch tips the graph walks: every ref's commit, and HEAD's.
fn tip_oids(refs: &refs::RefsSnapshot) -> FxHashSet<Oid> {
    refs.refs
        .iter()
        .map(|r| r.target.as_str())
        .chain(refs.head.as_deref())
        .filter_map(Oid::parse_hex)
        .collect()
}

/// The commits for `refs`, built from `base` by asking Git only for what it lacks. A tip that is
/// already in `base.store` brings nothing new, since the store holds every ancestor of every
/// commit in it; so the positives are the tips it lacks, and the negatives are the tips it holds,
/// old and new. An old tip that is no longer a tip (a moved or deleted branch) is a negative
/// only while it still exists, because `git log` stops with "bad object" on a commit that
/// `git gc` has pruned. Commits that no tip reaches any more (a deleted branch, the old commits
/// of a rewritten one) are then dropped. None when Git fails, when more than `limit` commits are
/// new or when the result would exceed `max`; the caller then loads the whole history.
async fn incremental_store(
    handle: &RepoHandle,
    base: &GraphSnapshot,
    refs: &refs::RefsSnapshot,
    limit: u32,
    max: u32,
) -> Option<Arc<CommitStore>> {
    let old_tips = tip_oids(&base.refs);
    let new_tips = tip_oids(refs);
    if old_tips == new_tips {
        return Some(base.store.clone());
    }
    let old = &base.store;
    // A tip that was already a tip but is not in the store is not a commit (git.git tags a
    // blob); there is nothing to walk from it.
    let positives: Vec<String> =
        new_tips.iter().filter(|t| old.index_of(t).is_none() && !old_tips.contains(t)).map(Oid::to_hex).collect();
    let records = if positives.is_empty() {
        Vec::new()
    } else {
        let mut negatives = Vec::new();
        let mut to_check = Vec::new();
        for tip in old_tips.union(&new_tips).filter(|t| old.index_of(t).is_some()) {
            if new_tips.contains(tip) {
                negatives.push(tip.to_hex());
            } else {
                to_check.push(tip.to_hex());
            }
        }
        let client = &handle.client;
        // The pinned `cat-file` saves a process start (a `wsl.exe` from Windows) when it is there.
        let missing = if to_check.is_empty() {
            Ok(Vec::new())
        } else if client.caps.batch_command {
            handle.catfile.missing(client, &handle.cancel, &to_check).await
        } else {
            log::missing_objects(client, &to_check).await
        };
        match missing {
            Ok(missing) => negatives.extend(to_check.into_iter().filter(|o| !missing.contains(o))),
            Err(e) => {
                tracing::warn!("incremental refresh could not check old tips, loading in full: {e}");
                return None;
            }
        }
        match log::get_log_between(client, &positives, &negatives, limit + 1).await {
            Ok(records) if records.len() as u32 > limit => {
                tracing::debug!("more than {limit} new commits, loading in full");
                return None;
            }
            Ok(records) => records,
            Err(e) => {
                tracing::warn!("incremental log failed, loading in full: {e}");
                return None;
            }
        }
    };
    // The new commits first, where a full `git log` would print them, so that commits made in
    // the same second sort the same way whichever load built the store.
    let mut store = CommitStore::new();
    for r in &records {
        store.insert(r);
    }
    for c in &old.commits {
        store.insert_data(c.clone(), old.author(c.author).clone());
    }
    let tips: Vec<Oid> = new_tips.into_iter().collect();
    let dropped = store.retain_reachable_from(&tips);
    if store.len() as u64 > u64::from(max) {
        return None;
    }
    tracing::debug!("incremental refresh: {} new commits, {} dropped, {} in all", records.len(), dropped.len(), store.len());
    Some(Arc::new(store))
}

/// A full `git log`, read as it streams. Every 2,000 commits `graph_progress` reports the count.
/// While nothing complete is on screen (a repository with no cache), what has arrived so far is
/// swapped in as a snapshot with `complete: false`: once at the first 2,000 commits, then every
/// 500 ms, or less often when building one takes long, so a large history shows its top rows
/// long before Git finishes. Returns the
/// store and whether `max` cut it short.
async fn stream_full(
    app: Option<&AppHandle>,
    handle: &Arc<RepoHandle>,
    options: GraphOptions,
    max: u32,
    refs: &Arc<refs::RefsSnapshot>,
    worktree: Option<WorktreeSummary>,
) -> Result<(CommitStore, bool)> {
    let mut shown = handle.graph.load_full();
    let mut partials = shown.as_ref().is_none_or(|s| !s.complete);
    let hidden = handle.hidden_refs();
    let mut store = CommitStore::new();
    let mut count = 0u32;
    let mut next_swap = Instant::now() + PARTIAL_EVERY;
    let mut on_record = |rec: log::CommitRecord| {
        store.insert(&rec);
        count += 1;
        if count.is_multiple_of(PROGRESS_EVERY) {
            if let Some(app) = app {
                let _ = GraphProgressEvent { repo_id: handle.key.clone(), loaded: count }.emit(app);
            }
        }
        // The first rows go up as soon as there is a screenful and more, then every 500 ms.
        let due = count == PROGRESS_EVERY || (count.is_multiple_of(256) && Instant::now() >= next_swap);
        if !partials || !due {
            return;
        }
        let started = Instant::now();
        let version = handle.graph_version.fetch_add(1, Ordering::Relaxed) + 1;
        let snap = Arc::new(GraphSnapshot::build(
            Arc::new(store.clone()),
            refs.clone(),
            worktree,
            options,
            version,
            false,
            false,
            &hidden,
        ));
        if swap_if_current(handle, &shown, &snap) {
            if let Some(app) = app {
                let _ = GraphSnapshotEvent::from_snapshot(&handle.key, &snap, SnapshotReason::Live).emit(app);
            }
            shown = Some(snap);
        } else {
            // Something complete landed meanwhile; leave it on screen.
            partials = false;
        }
        // Building takes longer as the store grows; keep it to a third of the load at most.
        next_swap = Instant::now() + PARTIAL_EVERY.max(started.elapsed() * 2);
    };
    let count = log::stream_log(&handle.client, max, &mut on_record).await?;
    Ok((store, count >= max))
}

/// Swap in `new` only while the handle still holds `expected` (the caller's own earlier
/// snapshot, or nothing), so that a snapshot another load swapped in meanwhile stays. True when
/// `new` was swapped in.
fn swap_if_current(handle: &RepoHandle, expected: &Option<Arc<GraphSnapshot>>, new: &Arc<GraphSnapshot>) -> bool {
    let previous = handle.graph.compare_and_swap(expected, Some(new.clone()));
    match (&*previous, expected) {
        (Some(a), Some(b)) => Arc::ptr_eq(a, b),
        (None, None) => true,
        _ => false,
    }
}

/// Paint from the cache if there is one. A large cache paints its top 500 rows first
/// (`complete: false`) and then loads whole. Returns None when nothing is cached, or when a live
/// snapshot landed first.
pub async fn load_from_cache(
    app: Option<&AppHandle>,
    db: &Arc<Db>,
    handle: &Arc<RepoHandle>,
    options: GraphOptions,
) -> Result<Option<Arc<GraphSnapshot>>> {
    let started = Instant::now();
    let hidden = handle.hidden_refs();
    let build = |cached: repo_cache::CachedGraph, complete: bool| {
        let version = handle.graph_version.fetch_add(1, Ordering::Relaxed) + 1;
        Arc::new(GraphSnapshot::build(
            Arc::new(cached.store),
            Arc::new(cached.refs),
            None,
            options,
            version,
            complete,
            cached.truncated,
            &hidden,
        ))
    };
    let emit = |snap: &GraphSnapshot| {
        if let Some(app) = app {
            let _ = GraphSnapshotEvent::from_snapshot(&handle.key, snap, SnapshotReason::Cache).emit(app);
        }
    };

    let key = handle.key.clone();
    let first = db.run(move |c| repo_cache::load_first_page(c, &key, FIRST_PAGE_ROWS, FIRST_PAGE_ABOVE)).await?;
    let mut shown = None;
    if let Some(page) = first {
        let snap = build(page, false);
        if !swap_if_current(handle, &None, &snap) {
            return Ok(None);
        }
        emit(&snap);
        tracing::debug!("cache first page for {} took {:?}", handle.key, started.elapsed());
        shown = Some(snap);
    }

    let key = handle.key.clone();
    let Some(cached) = db.run(move |c| repo_cache::load(c, &key)).await? else {
        return Ok(None);
    };
    let snap = build(cached, true);
    tracing::debug!("cache paint for {} with {} commits took {:?}", handle.key, snap.store.len(), started.elapsed());
    // Do not overwrite a live snapshot that landed first.
    if !swap_if_current(handle, &shown, &snap) {
        return Ok(None);
    }
    emit(&snap);
    Ok(Some(snap))
}

/// The open sequence: cache paint (if any), then a live load that persists.
pub async fn open_pipeline(
    app: &AppHandle,
    db: &Arc<Db>,
    handle: &Arc<RepoHandle>,
    options: GraphOptions,
    cache: &CacheSettings,
) {
    if let Err(e) = load_from_cache(Some(app), db, handle, options).await {
        tracing::warn!("cache load for {} failed: {e}", handle.key);
    }
    if let Err(e) = refresh_full(Some(app), Some(db), handle, options, cache, SnapshotReason::Live, false).await {
        tracing::warn!("live graph load for {} failed: {e}", handle.key);
    }
}

/// Status changed but refs did not: rebuild with or without the Working Tree
/// row, reusing the existing store and refs. Falls back to a full refresh
/// when there is no snapshot yet.
pub async fn refresh_status_only(
    app: Option<&AppHandle>,
    handle: &Arc<RepoHandle>,
    options: GraphOptions,
    cache: &CacheSettings,
) -> Result<Arc<GraphSnapshot>> {
    let Some(current) = handle.graph.load_full() else {
        return refresh_full(app, None, handle, options, cache, SnapshotReason::Status, false).await;
    };
    let status_snap = {
        let _shared = handle.lock.read().await;
        crate::git::queries::full_status(&handle.client, &handle.info.git_dir).await?
    };
    if status_snap.head_oid.as_deref() != current.refs.head.as_deref() {
        // HEAD moved: refs are stale too.
        return refresh_full(app, None, handle, options, cache, SnapshotReason::Refresh, false).await;
    }
    Ok(apply_status(app, handle, &current, status_snap, options).await)
}

/// Rebuild the current snapshot with the handle's hidden refs, from the commits already loaded:
/// no Git command runs and nothing is persisted. None when there is no snapshot yet; the first
/// load then uses the hidden refs by itself.
pub fn rebuild_filtered(app: Option<&AppHandle>, handle: &Arc<RepoHandle>, options: GraphOptions) -> Option<Arc<GraphSnapshot>> {
    let current = handle.graph.load_full()?;
    let version = handle.graph_version.fetch_add(1, Ordering::Relaxed) + 1;
    let snap = Arc::new(GraphSnapshot::build(
        current.store.clone(),
        current.refs.clone(),
        current.worktree,
        options,
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

/// Store the new status, and swap in a snapshot when the Working Tree row's
/// counts changed.
async fn apply_status(
    app: Option<&AppHandle>,
    handle: &Arc<RepoHandle>,
    current: &Arc<GraphSnapshot>,
    status_snap: status::StatusSnapshot,
    options: GraphOptions,
) -> Arc<GraphSnapshot> {
    let wt = worktree_summary(handle, &status_snap);
    if wt == current.worktree {
        *handle.last_status.write().await = Some(status_snap);
        return current.clone();
    }
    let version = handle.graph_version.fetch_add(1, Ordering::Relaxed) + 1;
    let snap = Arc::new(GraphSnapshot::build(
        current.store.clone(),
        current.refs.clone(),
        wt,
        options,
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
    fn git_out(dir: &PathBuf, args: &[&str]) -> String {
        let out = Command::new("git").arg("-C").arg(dir).args(args).output().expect("git runs");
        assert!(out.status.success(), "git {:?} failed", args);
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    }

    /// A throwaway repository; `commit` makes one commit at a clock that only moves forward, so
    /// no two commits share a time and both orders are fully determined.
    struct Scratch {
        dir: PathBuf,
        clock: i64,
    }

    impl Scratch {
        fn new(name: &str) -> Scratch {
            let dir = std::env::temp_dir().join(format!("stashpanda-{name}-{}", uuid::Uuid::new_v4().simple()));
            std::fs::create_dir_all(&dir).unwrap();
            git(&dir, &["init", "-q", "-b", "main"]);
            Scratch { dir, clock: 1_700_000_000 }
        }
        fn commit(&mut self, message: &str) -> String {
            self.clock += 60;
            // One file per first word of the message, so branches never conflict when merged.
            let file = format!("{}.txt", message.split(' ').next().unwrap_or("f"));
            std::fs::write(self.dir.join(file), format!("{message}\n")).unwrap();
            git(&self.dir, &["add", "-A"]);
            git_at(&self.dir, self.clock, &["commit", "-qm", message]);
            git_out(&self.dir, &["rev-parse", "HEAD"])
        }
        fn git(&self, args: &[&str]) {
            git_at(&self.dir, self.clock, args)
        }
        async fn open(&self) -> Arc<RepoHandle> {
            let entry = RepoEntry { key: "e2e".into(), path: self.dir.to_string_lossy().into_owned(), ..Default::default() };
            open_repo(None, &entry, &Settings::default(), &Host::detect(), None).await.expect("open")
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    /// Commit oids top to bottom, without the Working Tree row.
    fn commit_rows(snap: &GraphSnapshot) -> Vec<String> {
        snap.rows(0, snap.total()).into_iter().filter(|r| r.oid != "WORKTREE").map(|r| r.oid).collect()
    }

    /// What a full load from scratch shows for the repository as it is now.
    async fn full_rows(handle: &RepoHandle, options: GraphOptions) -> Vec<String> {
        let refs_snap = refs::get_refs(&handle.client).await.unwrap();
        let mut store = CommitStore::new();
        for r in log::get_log(&handle.client, 1000).await.unwrap() {
            store.insert(&r);
        }
        let snap = GraphSnapshot::build(Arc::new(store), Arc::new(refs_snap), None, options, 0, true, false, &Default::default());
        commit_rows(&snap)
    }

    #[tokio::test]
    async fn incremental_refresh_matches_a_full_load() {
        if which::which("git").is_err() {
            return;
        }
        let mut repo = Scratch::new("incremental");
        for i in 0..4 {
            repo.commit(&format!("main {i}"));
        }
        repo.git(&["tag", "v1", "HEAD~2"]);
        repo.git(&["checkout", "-qb", "topic", "HEAD~1"]);
        let topic_first = repo.commit("topic 1");
        repo.git(&["checkout", "-qb", "doomed", "main"]);
        let doomed = repo.commit("doomed 1");
        repo.git(&["checkout", "-q", "main"]);
        // A remote branch, so it can be force-pushed below.
        repo.git(&["checkout", "-q", "--detach", "main"]);
        repo.commit("feature 1");
        let feature_old = repo.commit("feature 2");
        repo.git(&["update-ref", "refs/remotes/origin/feature", &feature_old]);
        repo.git(&["checkout", "-q", "main"]);

        let handle = repo.open().await;
        let db = Db::open_in_memory().unwrap();
        let settings = Settings::default();
        let options = GraphOptions::default();
        let first = refresh_full(None, Some(&db), &handle, options, &settings.cache, SnapshotReason::Live, false).await.unwrap();
        assert_eq!(commit_rows(&first), full_rows(&handle, options).await);

        // Each change below: the incremental store applies (it does not fall back), and the
        // refresh that uses it shows exactly what a full load shows, and persists exactly that.
        async fn check(handle: &Arc<RepoHandle>, db: &Arc<Db>, what: &str) -> Arc<GraphSnapshot> {
            let settings = Settings::default();
            let options = GraphOptions::default();
            let base = handle.graph.load_full().unwrap();
            let refs_now = refs::get_refs(&handle.client).await.unwrap();
            let store = incremental_store(handle, &base, &refs_now, INCREMENTAL_LIMIT, 1000).await;
            assert!(store.is_some(), "{what}: the incremental path applies");
            let changes = refresh_if_changed(None, Some(db), handle, options, &settings.cache, true).await.unwrap();
            assert!(changes.refs, "{what}: refs changed");
            let snap = handle.graph.load_full().unwrap();
            assert_eq!(commit_rows(&snap), full_rows(handle, options).await, "{what}: same rows as a full load");
            let key = handle.key.clone();
            let cached = db.run(move |c| repo_cache::load(c, &key)).await.unwrap().unwrap();
            let mut in_cache: Vec<Oid> = cached.store.commits.iter().map(|c| c.oid).collect();
            let mut in_snap: Vec<Oid> = snap.store.commits.iter().map(|c| c.oid).collect();
            in_cache.sort();
            in_snap.sort();
            assert_eq!(in_cache, in_snap, "{what}: the cache holds the same commits");
            snap
        }

        // New commits on the checked-out branch.
        repo.commit("main 4");
        let main_5 = repo.commit("main 5");
        let snap = check(&handle, &db, "new commits").await;
        assert!(snap.find(&main_5).is_some());

        // A moved branch: topic gains a commit.
        repo.git(&["checkout", "-q", "topic"]);
        let topic_second = repo.commit("topic 2");
        repo.git(&["checkout", "-q", "main"]);
        let snap = check(&handle, &db, "moved branch").await;
        assert!(snap.find(&topic_second).is_some() && snap.find(&topic_first).is_some());

        // A force-pushed remote branch: its two commits are replaced, and the old ones pruned
        // from the object database, so the old tip cannot be a negative any more.
        repo.git(&["checkout", "-q", "--detach", "main"]);
        repo.commit("feature 1 again");
        let feature_new = repo.commit("feature 2 again");
        repo.git(&["update-ref", "refs/remotes/origin/feature", &feature_new]);
        repo.git(&["checkout", "-q", "main"]);
        repo.git(&["reflog", "expire", "--expire=now", "--all"]);
        repo.git(&["gc", "-q", "--prune=now"]);
        let pruned = log::missing_objects(&handle.client, std::slice::from_ref(&feature_old)).await.unwrap();
        assert_eq!(pruned, vec![feature_old.clone()], "the old tip is gone from the object database");
        let snap = check(&handle, &db, "force-pushed branch").await;
        assert!(snap.find(&feature_old).is_none() && snap.find(&feature_new).is_some());

        // A deleted branch: its only commit goes, with no `git log` at all.
        repo.git(&["branch", "-q", "-D", "doomed"]);
        let snap = check(&handle, &db, "deleted branch").await;
        assert!(snap.find(&doomed).is_none());

        // Fallbacks: more new commits than the limit, and a log that fails.
        repo.commit("main 6");
        repo.commit("main 7");
        let base = handle.graph.load_full().unwrap();
        let refs_now = refs::get_refs(&handle.client).await.unwrap();
        assert!(incremental_store(&handle, &base, &refs_now, 1, 1000).await.is_none(), "two new commits, limit one");
        assert!(incremental_store(&handle, &base, &refs_now, 2, 1000).await.is_some(), "two new commits, limit two");
        assert!(incremental_store(&handle, &base, &refs_now, 2, 5).await.is_none(), "more than the commit cap");
        let mut broken = refs_now.clone();
        broken.refs[0].target = "1234567890123456789012345678901234567890".into();
        assert!(incremental_store(&handle, &base, &broken, INCREMENTAL_LIMIT, 1000).await.is_none(), "bad object");
        // The refresh itself still lands, through the full load when it must.
        let changes = refresh_if_changed(None, Some(&db), &handle, options, &settings.cache, true).await.unwrap();
        assert!(changes.refs);
        assert_eq!(commit_rows(&handle.graph.load_full().unwrap()), full_rows(&handle, options).await);

        handle.close().await;
    }

    #[tokio::test]
    async fn topo_order_matches_git() {
        if which::which("git").is_err() {
            return;
        }
        // Two branches committed in turn, so their dates interleave, then merged; a third branch
        // left unmerged.
        let mut repo = Scratch::new("topo");
        repo.commit("base");
        repo.git(&["checkout", "-qb", "topic"]);
        repo.git(&["checkout", "-qb", "side"]);
        for i in 0..3 {
            repo.git(&["checkout", "-q", "main"]);
            repo.commit(&format!("main {i}"));
            repo.git(&["checkout", "-q", "topic"]);
            repo.commit(&format!("topic {i}"));
            repo.git(&["checkout", "-q", "side"]);
            repo.commit(&format!("side {i}"));
        }
        repo.git(&["checkout", "-q", "main"]);
        repo.clock += 60;
        repo.git(&["merge", "-q", "--no-ff", "-m", "merge topic", "topic"]);
        repo.commit("main after");

        let handle = repo.open().await;
        let options = GraphOptions { order: crate::settings::GraphOrder::Topo, ..GraphOptions::default() };
        let ours = full_rows(&handle, options).await;
        let git_topo = git_out(&repo.dir, &["log", "--topo-order", "--format=%H", "--branches", "--tags", "--remotes", "HEAD"]);
        let theirs: Vec<String> = git_topo.lines().map(String::from).collect();
        assert_eq!(ours, theirs, "the same order as git log --topo-order");
        let by_date = full_rows(&handle, GraphOptions::default()).await;
        assert_ne!(ours, by_date, "the fixture interleaves, so date order differs");
        handle.close().await;
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
        let snap = refresh_full(None, Some(&db), &handle, GraphOptions::default(), &settings.cache, SnapshotReason::Live, false).await.expect("refresh");
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
        let cached = load_from_cache(None, &db, &handle2, GraphOptions::default()).await.expect("cache").expect("present");
        let live: Vec<String> = snap.rows(1, 10).into_iter().map(|r| r.oid).collect();
        let from_cache: Vec<String> = cached.rows(0, 10).into_iter().map(|r| r.oid).collect();
        assert_eq!(live, from_cache, "cache order equals live order (minus the working tree row)");

        // Status-only refresh after cleaning the tree keeps the Working Tree row, now all zeros.
        std::fs::remove_file(dir.join("dirty.txt")).unwrap();
        let snap2 = refresh_status_only(None, &handle, GraphOptions::default(), &settings.cache).await.expect("status refresh");
        assert_eq!(snap2.total(), 7);
        assert_eq!(snap2.rows(0, 1)[0].worktree, Some(WorktreeSummary { staged: 0, unstaged: 0, untracked: 0, conflicted: 0 }));
        assert!(snap2.version > snap.version, "the counts changed, so the snapshot was rebuilt");

        // Nothing changed since the last refresh: no rebuild, same version.
        let none = refresh_if_changed(None, Some(&db), &handle, GraphOptions::default(), &settings.cache, true).await.expect("no-op refresh");
        assert_eq!(none, Changes::default());
        assert_eq!(handle.graph.load_full().unwrap().version, snap2.version);

        // A new untracked file: status changed, refs did not.
        std::fs::write(dir.join("new.txt"), "n\n").unwrap();
        let st = refresh_if_changed(None, Some(&db), &handle, GraphOptions::default(), &settings.cache, true).await.expect("status refresh");
        assert_eq!(st, Changes { status: true, refs: false });
        let snap3 = handle.graph.load_full().unwrap();
        assert_eq!(snap3.total(), 7);
        assert_eq!(snap3.rows(0, 1)[0].worktree.map(|w| w.untracked), Some(1));

        // A commit moves HEAD: full rebuild, even when only status was asked for.
        git(&dir, &["add", "-A"]);
        git_at(&dir, 1_700_001_000, &["commit", "-qm", "c5"]);
        let full = refresh_if_changed(None, Some(&db), &handle, GraphOptions::default(), &settings.cache, false).await.expect("full refresh");
        assert!(full.refs && full.status, "{full:?}");
        let snap4 = handle.graph.load_full().unwrap();
        assert_eq!(snap4.total(), 8, "7 commits and the clean Working Tree row");
        assert!(snap4.version > snap3.version);

        handle.close().await;
        handle2.close().await;
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Timings on a large repository named by `STASHPANDA_TIMING_REPO` (for example a clone of
    /// git.git). Not part of the normal run: `cargo test --lib time_large_repository -- --ignored --nocapture`.
    #[tokio::test]
    #[ignore]
    async fn time_large_repository() {
        let Ok(path) = std::env::var("STASHPANDA_TIMING_REPO") else { return };
        let settings = Settings::default();
        let entry = RepoEntry { key: "timing".into(), path, ..Default::default() };
        let host = Host::detect();
        let db_dir = std::env::temp_dir().join(format!("stashpanda-timing-{}", uuid::Uuid::new_v4().simple()));
        let db = Db::open(&db_dir.join("cache.sqlite")).unwrap();
        let options = GraphOptions::default();
        let ms = |t: std::time::Instant| format!("{:.1} ms", t.elapsed().as_secs_f64() * 1000.0);

        // A live load with nothing on screen: watch for the first partial snapshot while it streams.
        let handle = open_repo(None, &entry, &settings, &host, None).await.expect("open");
        let t = std::time::Instant::now();
        let watcher = {
            let handle = handle.clone();
            tokio::spawn(async move {
                let mut first: Option<(f64, u32)> = None;
                let mut partials = 0;
                let mut last_version = 0;
                loop {
                    if let Some(s) = handle.graph.load_full() {
                        if s.complete {
                            return (first, partials);
                        }
                        if s.version != last_version {
                            last_version = s.version;
                            partials += 1;
                            first.get_or_insert((t.elapsed().as_secs_f64() * 1000.0, s.total()));
                        }
                    }
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            })
        };
        let snap = refresh_full(None, None, &handle, options, &settings.cache, SnapshotReason::Live, true).await.expect("live load");
        println!("live load (refs, status, streamed git log, build): {} for {} rows, {} lanes", ms(t), snap.total(), snap.max_lanes);
        let (first, partials) = watcher.await.unwrap();
        if let Some((at, rows)) = first {
            println!("  first partial snapshot after {at:.1} ms with {rows} rows; {partials} partial snapshots in all");
        }
        let t = std::time::Instant::now();
        let to_save = snap.clone();
        db.run(move |c| repo_cache::persist(c, "timing", &to_save)).await.unwrap();
        println!("first persist: {}", ms(t));
        let t = std::time::Instant::now();
        let to_save = snap.clone();
        db.run(move |c| repo_cache::persist(c, "timing", &to_save)).await.unwrap();
        println!("second persist, nothing changed: {}", ms(t));

        let t = std::time::Instant::now();
        let cached = db.run(|c| repo_cache::load(c, "timing")).await.unwrap().expect("cached");
        println!("cache read from SQLite: {} for {} commits", ms(t), cached.store.len());
        let t = std::time::Instant::now();
        let built = GraphSnapshot::build(Arc::new(cached.store), Arc::new(cached.refs), None, options, 1, true, false, &Default::default());
        println!("snapshot build (order and lane sweep): {} for {} rows", ms(t), built.total());
        let t = std::time::Instant::now();
        let page = db.run(|c| repo_cache::load_first_page(c, "timing", FIRST_PAGE_ROWS, FIRST_PAGE_ABOVE)).await.unwrap().expect("first page");
        let page_snap = GraphSnapshot::build(Arc::new(page.store), Arc::new(page.refs), None, options, 1, false, false, &Default::default());
        println!("first page read and build: {} for {} rows", ms(t), page_snap.total());
        let same_top = page_snap.rows(0, 499).iter().zip(built.rows(0, 499)).all(|(a, b)| a == &b);
        println!("  the first page's top 499 rows equal the whole graph's: {same_top}");

        let handle2 = open_repo(None, &entry, &settings, &host, None).await.expect("open again");
        let t = std::time::Instant::now();
        load_from_cache(None, &db, &handle2, options).await.unwrap().expect("cache paint");
        println!("load_from_cache, first page then whole: {}", ms(t));
        let t = std::time::Instant::now();
        refresh_full(None, Some(&db), &handle2, options, &settings.cache, SnapshotReason::Live, false).await.unwrap();
        println!("live refresh after the cache paint, nothing changed: {}", ms(t));

        for start in [0u32, 40_000, built.total().saturating_sub(200)] {
            let t = std::time::Instant::now();
            let rows = built.rows(start, 200);
            let json = serde_json::to_string(&rows).unwrap();
            let edges: usize = rows.iter().map(|r| r.edges.len()).sum();
            println!("graph_rows page at {start}: {} including JSON ({} bytes, {} edges)", ms(t), json.len(), edges);
        }
        for (query, label) in [("merge", "common word"), ("Junio", "author"), ("zzzzqqqq", "no match")] {
            let t = std::time::Instant::now();
            let r = built.search(query, Some(10), crate::graph::snapshot::SearchDirection::Forward);
            println!("search for {label} '{query}': {} ({} matches)", ms(t), r.matches);
        }
        let topo = GraphOptions { order: crate::settings::GraphOrder::Topo, ..options };
        let t = std::time::Instant::now();
        let topo_snap = GraphSnapshot::build(built.store.clone(), built.refs.clone(), None, topo, 1, true, false, &Default::default());
        let json = serde_json::to_string(&topo_snap.rows(40_000, 200)).unwrap();
        println!("topological build: {}, {} lanes, page at 40000 is {} bytes", ms(t), topo_snap.max_lanes, json.len());

        // One new commit on a new branch: the incremental refresh against the whole history.
        if std::env::var("STASHPANDA_TIMING_COMMIT").is_ok() {
            let dir = PathBuf::from(&entry.path);
            let t = std::time::Instant::now();
            refresh_if_changed(None, Some(&db), &handle2, options, &settings.cache, true).await.unwrap();
            println!("refresh_if_changed with nothing changed (refs and status only): {}", ms(t));
            let tree_commit = git_out(&dir, &["commit-tree", "-m", "timing", "HEAD^{tree}", "-p", "HEAD"]);
            git(&dir, &["update-ref", "refs/heads/stashpanda-timing", &tree_commit]);
            let t = std::time::Instant::now();
            let changes = refresh_if_changed(None, Some(&db), &handle2, options, &settings.cache, true).await.unwrap();
            println!("refresh_if_changed after one new commit: {} (refs changed: {})", ms(t), changes.refs);
            git(&dir, &["branch", "-q", "-D", "stashpanda-timing"]);
            let t = std::time::Instant::now();
            refresh_if_changed(None, Some(&db), &handle2, options, &settings.cache, true).await.unwrap();
            println!("refresh_if_changed after deleting that branch: {}", ms(t));
        }

        handle.close().await;
        handle2.close().await;
        let _ = std::fs::remove_dir_all(&db_dir);
    }
}
