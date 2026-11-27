//! Builds and swaps a repository's graph snapshot; emits `graph_snapshot`.

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

use rustc_hash::FxHashSet;
use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::AppHandle;
use tauri_specta::Event;
use tokio_util::sync::CancellationToken;

use super::oid::Oid;
use super::snapshot::{GraphSnapshot, WorktreeSummary};
use super::store::CommitStore;
use crate::error::{GitError, Result};
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

/// The live load started when a tab opened failed (the connection dropped, Git failed), so no
/// snapshot follows; the frontend stops showing it as loading and offers to try again.
#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
#[serde(rename_all = "camelCase")]
pub struct GraphLoadFailedEvent {
    pub repo_id: String,
    pub error: GitError,
}

/// How long a repository's graph may be worked on before the frontend is told that it is busy.
const BUSY_AFTER: Duration = Duration::from_secs(2);

/// Sent with `busy: true` once working out a repository's graph has taken longer than two
/// seconds, counted from when a load, refresh or rebuild started, or started to wait for another
/// one, and with `busy: false` when none is running or waiting any more. Whatever is asked for
/// meanwhile (a refresh, a hidden ref, a ref's colour) waits its turn for
/// `RepoHandle::graph_build`, so the frontend shows that the graph may not follow at once.
#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
#[serde(rename_all = "camelCase")]
pub struct GraphBusyEvent {
    pub repo_id: String,
    pub busy: bool,
}

/// How many loads, refreshes and rebuilds of a repository's graph are running or waiting, and
/// whether the frontend has been told that it is busy (`GraphBusyEvent`).
#[derive(Debug, Default)]
pub struct GraphBusy {
    pub working: u32,
    pub announced: bool,
}

/// One load, refresh or rebuild, counted in `RepoHandle::graph_busy` from when it starts, or
/// starts to wait, until it is dropped.
struct Working {
    handle: Arc<RepoHandle>,
    app: Option<AppHandle>,
    done: CancellationToken,
}

impl Working {
    /// Counts the work in, and announces the repository as busy if the work is still there
    /// after `delay`.
    fn start(app: Option<&AppHandle>, handle: &Arc<RepoHandle>, delay: Duration) -> Working {
        handle.graph_busy.lock().expect("graph busy lock").working += 1;
        let work = Working { handle: handle.clone(), app: app.cloned(), done: CancellationToken::new() };
        let (handle, app, done) = (work.handle.clone(), work.app.clone(), work.done.clone());
        tokio::spawn(async move {
            tokio::select! {
                _ = done.cancelled() => return,
                _ = tokio::time::sleep(delay) => {}
            }
            let mut busy = handle.graph_busy.lock().expect("graph busy lock");
            // `drop` cancels under this lock, so work that ended this instant announces nothing.
            if done.is_cancelled() || busy.announced {
                return;
            }
            busy.announced = true;
            announce_busy(app.as_ref(), &handle, true);
        });
        work
    }
}

impl Drop for Working {
    fn drop(&mut self) {
        let mut busy = self.handle.graph_busy.lock().expect("graph busy lock");
        self.done.cancel();
        busy.working -= 1;
        if busy.working == 0 && busy.announced {
            busy.announced = false;
            announce_busy(self.app.as_ref(), &self.handle, false);
        }
    }
}

fn announce_busy(app: Option<&AppHandle>, handle: &RepoHandle, busy: bool) {
    if let Some(app) = app {
        let _ = GraphBusyEvent { repo_id: handle.key.clone(), busy }.emit(app);
    }
}

/// What everything that stores a snapshot holds while it works: `RepoHandle::graph_build`, and
/// its count in `RepoHandle::graph_busy`.
struct Building<'a> {
    _lock: tokio::sync::MutexGuard<'a, ()>,
    _work: Working,
}

/// Waits for `graph_build`. The two seconds before the repository is announced as busy start
/// here, before the wait, so a click that waits behind a long load counts its own waiting.
async fn start_building<'a>(app: Option<&AppHandle>, handle: &'a Arc<RepoHandle>) -> Building<'a> {
    let work = Working::start(app, handle, BUSY_AFTER);
    Building { _lock: handle.graph_build.lock().await, _work: work }
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
///
/// Like everything here that stores a snapshot, it holds `RepoHandle::graph_build` from before
/// it reads Git until the snapshot is stored. Two loads at once would otherwise each read the
/// refs and store when done, and the one that read first could store last: a slow load of the
/// whole history would put its older refs on top of what a quick refresh had just stored.
///
/// The cache is written last, with both locks released (`save_to_cache`).
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
    let snap = {
        let _building = start_building(app, handle).await;
        let _shared = handle.lock.read().await;
        load_live(app, handle, options, cache, reason, force_full).await?
    };
    save_to_cache(db, handle, &snap).await;
    Ok(snap)
}

/// The live load itself, for a caller that already holds `graph_build` and `lock`. It writes
/// nothing to the cache.
async fn load_live(
    app: Option<&AppHandle>,
    handle: &Arc<RepoHandle>,
    options: GraphOptions,
    cache: &CacheSettings,
    reason: SnapshotReason,
    force_full: bool,
) -> Result<Arc<GraphSnapshot>> {
    let (refs_res, status_res) = crate::git::queries::refs_and_status(&handle.client, &handle.info.git_dir).await;
    rebuild(app, handle, options, cache, reason, refs_res?, status_res?, force_full).await
}

/// Writes `snap` to the cache, so the next start paints it before Git answers. Called once the
/// snapshot is stored and shown, with `graph_build` and `lock` released: the first write for a
/// long history takes seconds, and held under those locks it kept the repository marked as busy,
/// a hidden ref waiting and every write to the repository blocked, all for a file nothing reads
/// until the next start. Writes go one at a time (`RepoHandle::cache_saved`), and an older
/// snapshot is never written after a newer one: versions are given out under `graph_build`, so
/// the higher one is the later picture.
async fn save_to_cache(db: Option<&Arc<Db>>, handle: &Arc<RepoHandle>, snap: &Arc<GraphSnapshot>) {
    let Some(db) = db else { return };
    let mut saved = handle.cache_saved.lock().await;
    if snap.version <= *saved {
        return;
    }
    let key = handle.key.clone();
    let to_save = snap.clone();
    let started = std::time::Instant::now();
    match db.run(move |c| repo_cache::persist(c, &key, &to_save)).await {
        Ok(()) => {
            *saved = snap.version;
            tracing::debug!("persisted {} commits for {} in {:?}", snap.store.len(), handle.key, started.elapsed());
        }
        Err(e) => tracing::warn!("could not persist graph cache for {}: {e}", handle.key),
    }
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
    let (changes, rebuilt) = {
        let _building = start_building(app, handle).await;
        let _shared = handle.lock.read().await;
        changed_parts(app, handle, options, cache, check_refs).await?
    };
    if let Some(snap) = rebuilt {
        save_to_cache(db, handle, &snap).await;
    }
    Ok(changes)
}

/// `refresh_if_changed` for a caller that holds `graph_build` and `lock`: what changed, and the
/// snapshot to write to the cache when the commits or refs did.
async fn changed_parts(
    app: Option<&AppHandle>,
    handle: &Arc<RepoHandle>,
    options: GraphOptions,
    cache: &CacheSettings,
    check_refs: bool,
) -> Result<(Changes, Option<Arc<GraphSnapshot>>)> {
    let client = &handle.client;
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
        let snap = rebuild(app, handle, options, cache, SnapshotReason::Refresh, refs_snap, status_snap, false).await?;
        return Ok((Changes { status: status_changed, refs: true }, Some(snap)));
    }
    if status_changed {
        if let Some(c) = &current {
            apply_status(app, handle, c, status_snap, options).await;
        }
    }
    Ok((Changes { status: status_changed, refs: false }, None))
}

/// Log, build, swap and emit. The caller holds `graph_build` and the shared lock, and writes
/// the snapshot to the cache once it has released them (`save_to_cache`).
#[allow(clippy::too_many_arguments)]
async fn rebuild(
    app: Option<&AppHandle>,
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
    let snap = Arc::new(GraphSnapshot::build_coloured(
        store,
        refs_snap,
        worktree,
        handle.graph_options(options),
        version,
        true,
        truncated,
        &handle.hidden_refs(),
        &handle.ref_colours(),
    ));
    handle.graph.store(Some(snap.clone()));
    *handle.last_status.write().await = Some(status_snap);

    if let Some(app) = app {
        let _ = GraphSnapshotEvent::from_snapshot(&handle.key, &snap, reason).emit(app);
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
    let colours = handle.ref_colours();
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
        let snap = Arc::new(GraphSnapshot::build_coloured(
            Arc::new(store.clone()),
            refs.clone(),
            worktree,
            handle.graph_options(options),
            version,
            false,
            false,
            &hidden,
            &colours,
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
    let count = log::stream_log(&handle.client, max, refs.head.is_some(), &mut on_record).await?;
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
    // A refresh the watcher asks for meanwhile waits for the paint, then starts from the cache.
    let _building = start_building(app, handle).await;
    let started = Instant::now();
    let hidden = handle.hidden_refs();
    let colours = handle.ref_colours();
    let build = |cached: repo_cache::CachedGraph, complete: bool| {
        let version = handle.graph_version.fetch_add(1, Ordering::Relaxed) + 1;
        Arc::new(GraphSnapshot::build_coloured(
            Arc::new(cached.store),
            Arc::new(cached.refs),
            None,
            handle.graph_options(options),
            version,
            complete,
            cached.truncated,
            &hidden,
            &colours,
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
        // A closed tab has nobody to tell.
        if !handle.cancel.is_cancelled() {
            let _ = GraphLoadFailedEvent { repo_id: handle.key.clone(), error: e }.emit(app);
        }
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
    // Taken before `current` is read: the status call below takes a while, and a snapshot stored
    // during it would be replaced by one built from the older `current`.
    let _building = start_building(app, handle).await;
    let _shared = handle.lock.read().await;
    let Some(current) = handle.graph.load_full() else {
        return load_live(app, handle, options, cache, SnapshotReason::Status, false).await;
    };
    let status_snap = crate::git::queries::full_status(&handle.client, &handle.info.git_dir).await?;
    if status_snap.head_oid.as_deref() != current.refs.head.as_deref() {
        // HEAD moved: refs are stale too.
        return load_live(app, handle, options, cache, SnapshotReason::Refresh, false).await;
    }
    Ok(apply_status(app, handle, &current, status_snap, options).await)
}

/// Rebuild the current snapshot with the handle's hidden refs, from the commits already loaded:
/// no Git command runs and nothing is persisted. None when there is no snapshot yet; the first
/// load then uses the hidden refs by itself. It waits for a load that is running, and then
/// rebuilds what that load stored; rebuilding the snapshot from before it would drop the load's
/// new commits.
pub async fn rebuild_filtered(app: Option<&AppHandle>, handle: &Arc<RepoHandle>, options: GraphOptions) -> Option<Arc<GraphSnapshot>> {
    let _building = start_building(app, handle).await;
    let current = handle.graph.load_full()?;
    let version = handle.graph_version.fetch_add(1, Ordering::Relaxed) + 1;
    let snap = Arc::new(GraphSnapshot::build_coloured(
        current.store.clone(),
        current.refs.clone(),
        current.worktree,
        handle.graph_options(options),
        version,
        current.complete,
        current.truncated,
        &handle.hidden_refs(),
        &handle.ref_colours(),
    ));
    handle.graph.store(Some(snap.clone()));
    if let Some(app) = app {
        let _ = GraphSnapshotEvent::from_snapshot(&handle.key, &snap, SnapshotReason::Filter).emit(app);
    }
    Some(snap)
}

/// Store the new status, and swap in a snapshot when the Working Tree row's
/// counts changed. The caller holds `graph_build`, and read `current` while holding it.
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
    let snap = Arc::new(GraphSnapshot::build_coloured(
        current.store.clone(),
        current.refs.clone(),
        wt,
        handle.graph_options(options),
        version,
        current.complete,
        current.truncated,
        &handle.hidden_refs(),
        &handle.ref_colours(),
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
    async fn signed_commits_and_a_branch_with_no_commits_load_in_full() {
        if which::which("git").is_err() || which::which("ssh-keygen").is_err() {
            return;
        }
        let mut repo = Scratch::new("signed");
        let first = repo.commit("main 1");
        // A commit signed with an SSH key, and `log.showSignature`, which makes `git log` print
        // the check of each signed commit before its record.
        let key = repo.dir.with_extension("key");
        let st = Command::new("ssh-keygen").args(["-q", "-t", "ed25519", "-N", "", "-C", "t@t", "-f"]).arg(&key).status().unwrap();
        assert!(st.success());
        repo.clock += 60;
        let signing_key = format!("user.signingkey={}", key.display());
        repo.git(&["-c", "gpg.format=ssh", "-c", &signing_key, "commit", "-q", "-S", "--allow-empty", "-m", "signed"]);
        let signed = git_out(&repo.dir, &["rev-parse", "HEAD"]);
        repo.git(&["config", "log.showSignature", "true"]);

        let handle = repo.open().await;
        let settings = Settings::default();
        let load = || refresh_full(None, None, &handle, GraphOptions::default(), &settings.cache, SnapshotReason::Live, true);
        assert_eq!(commit_rows(&load().await.unwrap()), [signed.clone(), first.clone()]);
        // A branch with no commits yet: HEAD names no commit, and main's commits still show.
        repo.git(&["checkout", "-q", "--orphan", "fresh"]);
        assert_eq!(commit_rows(&load().await.unwrap()), [signed, first]);
        handle.close().await;
        let _ = std::fs::remove_file(&key);
        let _ = std::fs::remove_file(key.with_extension("key.pub"));
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

    /// A repository that changed while it was closed: the cache paints what it was, and the live
    /// load that follows shows what it is now, starting from the cached commits.
    #[tokio::test]
    async fn live_load_over_a_stale_cache_shows_the_repository_as_it_is_now() {
        if which::which("git").is_err() {
            return;
        }
        let mut repo = Scratch::new("stale-cache");
        for i in 0..3 {
            repo.commit(&format!("main {i}"));
        }
        repo.git(&["tag", "v1"]);
        repo.git(&["update-ref", "refs/remotes/origin/main", "HEAD"]);
        repo.git(&["checkout", "-qb", "dev"]);
        let old_head = repo.commit("dev 1");

        let settings = Settings::default();
        let options = GraphOptions::default();
        let db = Db::open_in_memory().unwrap();
        let handle = repo.open().await;
        refresh_full(None, Some(&db), &handle, options, &settings.cache, SnapshotReason::Live, false).await.unwrap();
        handle.close().await;

        // Meanwhile, outside the app: more commits, a branch merged, another branch checked out
        // with a commit of its own, an amended commit, and the remote branch moved.
        repo.commit("dev 2");
        repo.git(&["checkout", "-qb", "topic"]);
        repo.commit("topic 1");
        repo.git(&["checkout", "-q", "dev"]);
        repo.clock += 60;
        repo.git(&["merge", "-q", "--no-ff", "-m", "merge topic", "topic"]);
        repo.git(&["update-ref", "refs/remotes/origin/main", "HEAD~1"]);
        repo.git(&["checkout", "-qb", "feature", "HEAD~1"]);
        repo.commit("feature 1");
        repo.clock += 60;
        repo.git(&["commit", "-q", "--amend", "-m", "feature 1 amended"]);
        let new_head = git_out(&repo.dir, &["rev-parse", "HEAD"]);

        let handle = repo.open().await;
        let cached = load_from_cache(None, &db, &handle, options).await.unwrap().expect("a cache to paint from");
        assert_eq!(cached.refs.head.as_deref(), Some(old_head.as_str()), "the cache still has the old HEAD");
        assert_eq!(cached.refs.head_ref.as_deref(), Some("refs/heads/dev"));

        let live = refresh_full(None, Some(&db), &handle, options, &settings.cache, SnapshotReason::Live, false).await.unwrap();
        assert!(live.version > cached.version);
        assert_eq!(live.refs.head.as_deref(), Some(new_head.as_str()));
        assert_eq!(live.refs.head_ref.as_deref(), Some("refs/heads/feature"));
        assert_eq!(commit_rows(&live), full_rows(&handle, options).await, "the same rows as a load from nothing");
        let rows = live.rows(0, 3);
        assert_eq!(rows[0].kind, crate::graph::snapshot::RowKind::WorkingTree);
        assert!(rows[1].is_head && rows[1].refs.iter().any(|l| l.short == "feature"), "the checked-out branch is on the first commit row");
        handle.close().await;
    }

    /// `count` commits on main, one on top of the other, written by `git fast-import` so that a
    /// history long enough to take a while to read is made in about a second.
    fn import_commits(dir: &PathBuf, count: u32) {
        use std::io::Write;
        let mut stream = String::new();
        for i in 1..=count {
            let message = format!("imported {i}");
            stream.push_str(&format!(
                "commit refs/heads/main\nmark :{i}\ncommitter T <t@t> {} +0000\ndata {}\n{message}\n",
                1_600_000_000 + i64::from(i) * 60,
                message.len(),
            ));
            if i > 1 {
                stream.push_str(&format!("from :{}\n", i - 1));
            }
            stream.push('\n');
        }
        let mut child = Command::new("git")
            .arg("-C").arg(dir)
            .args(["fast-import", "--quiet"])
            .stdin(std::process::Stdio::piped())
            .spawn()
            .expect("git fast-import runs");
        child.stdin.take().unwrap().write_all(stream.as_bytes()).unwrap();
        assert!(child.wait().unwrap().success(), "git fast-import failed");
    }

    /// Two loads at once: a slow one that reads the whole history again (the toolbar's Refresh)
    /// and, started while it is still reading, a quick one for a branch made meanwhile (the
    /// watcher's). The slow one read the refs before the branch existed and finishes last, so
    /// without something to put the two in order it would store its older picture on top of
    /// the newer one, and the branch would be missing until the next change.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_slow_load_never_replaces_a_newer_snapshot() {
        if which::which("git").is_err() {
            return;
        }
        let repo = Scratch::new("slow-load");
        import_commits(&repo.dir, 30_000);
        let handle = repo.open().await;
        let settings = Settings::default();
        let options = GraphOptions::default();
        refresh_full(None, None, &handle, options, &settings.cache, SnapshotReason::Live, false).await.unwrap();

        for round in 0..3 {
            let branch = format!("made-meanwhile-{round}");
            let slow = {
                let (handle, cache) = (handle.clone(), settings.cache.clone());
                tokio::spawn(async move { refresh_full(None, None, &handle, options, &cache, SnapshotReason::Refresh, true).await.map(|_| ()) })
            };
            // Long enough for the slow load to have read the refs and be reading the log.
            tokio::time::sleep(Duration::from_millis(40)).await;
            repo.git(&["branch", &branch, "HEAD~3"]);
            refresh_if_changed(None, None, &handle, options, &settings.cache, true).await.unwrap();
            slow.await.unwrap().unwrap();

            let snap = handle.graph.load_full().unwrap();
            assert!(snap.refs.refs.iter().any(|r| r.short == branch), "round {round}: the snapshot kept has the branch made during the slow load");
            assert_eq!(snap.version, handle.graph_version.load(Ordering::Relaxed), "round {round}: the snapshot kept is the one numbered last");
        }
        handle.close().await;
    }

    /// A rebuild from the commits in memory (a ref hidden or shown, a ref's colour) asked for
    /// while a load is running waits for it, and then rebuilds what the load stored. Rebuilding
    /// the snapshot from before the load would put that one back, without the load's commits.
    #[tokio::test]
    async fn a_rebuild_from_memory_waits_for_a_running_load() {
        if which::which("git").is_err() {
            return;
        }
        let mut repo = Scratch::new("rebuild-waits");
        repo.commit("main 1");
        let handle = repo.open().await;
        let settings = Settings::default();
        let options = GraphOptions::default();
        refresh_full(None, None, &handle, options, &settings.cache, SnapshotReason::Live, false).await.unwrap();

        // The test plays the load: it holds the lock a load holds, and stores only further down.
        let building = handle.graph_build.lock().await;
        let newer = repo.commit("main 2");
        let rebuilt = {
            let handle = handle.clone();
            tokio::spawn(async move { rebuild_filtered(None, &handle, options).await })
        };
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!rebuilt.is_finished(), "the rebuild waits while a load holds the lock");
        {
            let _shared = handle.lock.read().await;
            load_live(None, &handle, options, &settings.cache, SnapshotReason::Refresh, false).await.unwrap();
        }
        drop(building);

        let snap = rebuilt.await.unwrap().expect("a snapshot to rebuild");
        assert!(snap.find(&newer).is_some(), "the rebuild starts from what the load stored");
        assert_eq!(handle.graph.load_full().unwrap().version, snap.version);
        assert_eq!(snap.version, handle.graph_version.load(Ordering::Relaxed), "the snapshot kept is the one numbered last");
        handle.close().await;
    }

    /// The busy state the frontend is told about: nothing for work that ends before the delay,
    /// busy once work has lasted longer, and clear again only when the last piece of work that
    /// was running or waiting has ended.
    #[tokio::test]
    async fn a_graph_worked_on_for_long_is_announced_as_busy_until_all_work_ends() {
        if which::which("git").is_err() {
            return;
        }
        let mut repo = Scratch::new("busy");
        repo.commit("main 1");
        let handle = repo.open().await;
        let delay = Duration::from_millis(60);
        let state = |handle: &RepoHandle| {
            let busy = handle.graph_busy.lock().unwrap();
            (busy.working, busy.announced)
        };

        // Quick work: over before the delay, and never announced, not even once the delay has passed.
        let quick = Working::start(None, &handle, delay);
        assert_eq!(state(&handle), (1, false));
        drop(quick);
        tokio::time::sleep(delay * 2).await;
        assert_eq!(state(&handle), (0, false));

        // Long work is announced after the delay. A second piece that starts meanwhile (in the
        // app it would be waiting for the lock) keeps the repository busy after the first ends.
        let long = Working::start(None, &handle, delay);
        tokio::time::sleep(delay * 2).await;
        assert_eq!(state(&handle), (1, true));
        let waiting = Working::start(None, &handle, delay);
        drop(long);
        assert_eq!(state(&handle), (1, true));
        drop(waiting);
        assert_eq!(state(&handle), (0, false));

        // A real refresh counts itself in and out.
        let settings = Settings::default();
        refresh_full(None, None, &handle, GraphOptions::default(), &settings.cache, SnapshotReason::Live, false).await.unwrap();
        assert_eq!(state(&handle), (0, false));
        handle.close().await;
    }

    /// The cache is written after the snapshot is stored and with nothing held: while a write
    /// is under way the graph can be rebuilt, the repository written to, and it is not busy.
    #[tokio::test]
    async fn the_cache_is_written_with_the_graph_and_the_repository_free() {
        if which::which("git").is_err() {
            return;
        }
        let mut repo = Scratch::new("cache-outside");
        repo.commit("main 1");
        let handle = repo.open().await;
        let settings = Settings::default();
        let options = GraphOptions::default();
        let db = Db::open_in_memory().unwrap();

        // The test holds what a cache write holds, so the refresh below stops just before its own.
        let writing = handle.cache_saved.lock().await;
        let refresh = {
            let (handle, db, cache) = (handle.clone(), db.clone(), settings.cache.clone());
            tokio::spawn(async move { refresh_full(None, Some(&db), &handle, options, &cache, SnapshotReason::Live, false).await.map(|s| s.version) })
        };
        for _ in 0..200 {
            if handle.graph.load_full().is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(handle.graph.load_full().is_some_and(|s| s.complete), "the snapshot is stored before the cache is written");
        assert!(!refresh.is_finished(), "the refresh is waiting to write the cache");
        assert!(handle.graph_build.try_lock().is_ok(), "the graph can be rebuilt meanwhile");
        assert!(handle.lock.try_write().is_ok(), "the repository can be written to meanwhile");
        assert_eq!(handle.graph_busy.lock().unwrap().working, 0, "and it does not count as busy");
        let key = handle.key.clone();
        assert!(db.run(move |c| repo_cache::load(c, &key)).await.unwrap().is_none(), "nothing is in the cache yet");

        drop(writing);
        let version = refresh.await.unwrap().unwrap();
        assert_eq!(*handle.cache_saved.lock().await, version);
        let key = handle.key.clone();
        assert_eq!(db.run(move |c| repo_cache::load(c, &key)).await.unwrap().unwrap().store.len(), 1);
        handle.close().await;
    }

    /// Two cache writes can be waiting at once, in either order. The older snapshot is not
    /// written over the newer one.
    #[tokio::test]
    async fn an_older_snapshot_is_never_written_to_the_cache_after_a_newer_one() {
        if which::which("git").is_err() {
            return;
        }
        let mut repo = Scratch::new("cache-order");
        repo.commit("main 1");
        let handle = repo.open().await;
        let settings = Settings::default();
        let options = GraphOptions::default();
        let db = Db::open_in_memory().unwrap();
        let older = refresh_full(None, None, &handle, options, &settings.cache, SnapshotReason::Live, false).await.unwrap();
        repo.commit("main 2");
        let newer = refresh_full(None, None, &handle, options, &settings.cache, SnapshotReason::Refresh, false).await.unwrap();
        assert!(newer.version > older.version);

        save_to_cache(Some(&db), &handle, &newer).await;
        save_to_cache(Some(&db), &handle, &older).await;
        assert_eq!(*handle.cache_saved.lock().await, newer.version);
        let key = handle.key.clone();
        let cached = db.run(move |c| repo_cache::load(c, &key)).await.unwrap().unwrap();
        assert_eq!(cached.store.len(), 2, "the cache holds the newer snapshot's two commits");
        handle.close().await;
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
