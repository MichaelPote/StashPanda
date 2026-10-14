import { computed, markRaw, ref, shallowReactive, watch, type Ref } from "vue";
import { defineStore } from "pinia";
import {
  commands,
  events,
  type Activity,
  type Category,
  type ConsoleEvent,
  type DiffTarget,
  type GitError,
  type GraphMeta,
  type GraphProgressEvent,
  type GraphRow,
  type GraphSnapshotEvent,
  type OperationProgress,
  type RefsSnapshot,
  type RepoChangedEvent,
  type RepoInfo,
  type StashEntry,
  type StatusSnapshot,
  type TransportState,
  type GraphLoadFailedEvent,
  type ConnectionState,
  type WatcherKind,
  type WatcherStateEvent,
} from "@/lib/bindings";
import { DEFAULT_COMPARISON, type ConflictComparison } from "@/lib/conflicts";
import { errorMessage, IpcError, unwrap } from "@/lib/ipc";
import { ROW_HEIGHT } from "@/lib/graph/paths";
import { rowAt } from "@/composables/useGraphPages";
import { useSettingsStore } from "@/stores/settings";
import { useUiStateStore } from "@/stores/uiState";

/** One console row. A `started` and a `finished` event with the same `seq` merge into one entry. */
export interface ConsoleEntry {
  seq: number;
  category: Category;
  label: string;
  argv: string[];
  display: string;
  /** Just the git arguments the app asked for (`checkout master`), without the wrapper. */
  command: string;
  startedAtMs: number;
  durationMs: number | null;
  exitCode: number | null;
  running: boolean;
  stdout: string;
  stderr: string;
  stdoutBytes: number;
  stderrBytes: number;
  stdoutTruncated: boolean;
  stderrTruncated: boolean;
  /** A successful internal or watcher command: the backend sent its byte counts but not its text. */
  outputOmitted: boolean;
  error: GitError | null;
}

/** Page cache for one repository's commit graph. `pages` and `inflight` are raw (never deeply reactive). */
export interface GraphState {
  meta: GraphMeta | null;
  /** Page number (index / 200) -> rows. Held via `markRaw`; bump `pagesVersion` after changing it. */
  pages: Map<number, GraphRow[]>;
  /** Incremented whenever `pages` changes, so components re-read the raw Map. */
  pagesVersion: Ref<number>;
  /** Page number -> the fetch that will fill it. */
  inflight: Map<number, Promise<void>>;
  /** True while a snapshot is still being built in the backend. */
  loading: boolean;
  /** Commits loaded so far, from `graph-progress-event`. */
  progress: number;
}

export interface Selection {
  /** Selected rows, by oid (the Working Tree row is `"WORKTREE"`). Replaced, never mutated. */
  oids: Set<string>;
  /** Start of a Shift-range, and the row the keyboard moves from. */
  anchorOid: string | null;
}

/** A button rendered next to an error notice; `run` is called when it is clicked and the notice is dismissed. */
export interface NoticeAction {
  label: string;
  run: () => void | Promise<void>;
}

/** The SSH connection's state as the status bar shows it. */
export interface SessionConnection {
  state: ConnectionState;
  detail: string | null;
  /** For a lost connection: when, in `Date.now()` time, the next attempt is due. */
  retryAt: number | null;
}

/** A fetch, pull or push in progress in one repository. Replaced, never mutated. */
export interface RunningOperation {
  /** The id the operation was started with; `opCancel` takes it. */
  opId: string;
  /** What is happening, such as `Fetching from every remote`. */
  title: string;
  /** Git's latest progress update, such as `Receiving objects:  45% (450/1000)`; empty at first. */
  line: string;
  /** False once the operation has reached a step that cannot be stopped. */
  cancellable: boolean;
  /** Stop was pressed and the operation has not finished yet. */
  stopping: boolean;
}

/** Rows selected in the Files panel, by `FileItem.id`. Replaced, never mutated. */
export interface FileSelection {
  ids: Set<string>;
  /** Start of a Shift-range. */
  anchorId: string | null;
}

export function emptyFileSelection(): FileSelection {
  return { ids: new Set<string>(), anchorId: null };
}

export interface SelectedFile {
  path: string;
  oldPath?: string | null;
  target: DiffTarget;
}

export interface RepoSession {
  key: string;
  info: RepoInfo;
  /** Shallow-reactive ring buffer, shared with the detached buffer created before `repoOpen` returned. */
  consoleEntries: ConsoleEntry[];
  graph: GraphState;
  status: StatusSnapshot | null;
  refs: RefsSnapshot | null;
  /** `git stash list`, newest first. Null until the first load. */
  stashes: StashEntry[] | null;
  selection: Selection;
  /** The row that drives the Files panel: the most recently selected commit or the Working Tree row. */
  selectedRow: GraphRow | null;
  /** The file whose diff is shown: the last file clicked, whatever else is selected. */
  selectedFile: SelectedFile | null;
  fileSelection: FileSelection;
  /** Commit message typed into the Commit dialog and not committed yet. */
  commitDraft: string;
  /** Full names of the refs switched off in the commit graph. Replaced, never mutated. */
  hiddenRefs: Set<string>;
  /** Saved scroll offset of the commit list, so it survives tab switches and snapshot swaps. */
  scrollTop: number;
  /** A row index the graph view should scroll into view (set by the Branches panel). */
  reveal: { index: number } | null;
  /**
   * The commit selected when this repository was last open (`perRepo[key].lastSelectedOid`),
   * until the graph holds it and it is selected again, or the first complete graph lacks it.
   */
  restoreOid: string | null;
  /** The Find box over the Commits panel (Ctrl+F): whether it shows, and the text in it. */
  findOpen: boolean;
  findQuery: string;
  /** Bumped to move the keyboard focus into the Find box, even when it is already open. */
  findFocus: number;
  watcherKind: WatcherKind | null;
  /** Why the watcher is what it is, when that needs saying ("inotifywait is not installed on …"). */
  watcherDetail: string | null;
  /** The SSH connection's latest state, for repositories reached over SSH; null until one arrives. */
  connection: SessionConnection | null;
  /** When the last fetch from this tab finished (`Date.now()` time), or when the tab opened. Automatic fetching counts from it. */
  lastFetchAt: number;
  /** An automatic fetch failed and the user was told; later failures stay quiet until one works. */
  autoFetchFailed: boolean;
  /**
   * The stash (`stash@{0}`) whose apply or pop stopped on conflicts, which names that side of
   * them; cleared once no file is conflicted.
   */
  stashConflict: string | null;
  /** The fetch, pull or push running in this repository, shown in the status bar. */
  operation: RunningOperation | null;
  /** Which two versions of a conflicted file the diff panel compares; kept from one file to the next. */
  conflictComparison: ConflictComparison;
}

function toEntry(ev: ConsoleEvent): ConsoleEntry {
  return {
    seq: ev.seq,
    category: ev.category,
    label: ev.label,
    argv: ev.argv,
    display: ev.display,
    command: ev.command,
    startedAtMs: ev.startedAtMs,
    durationMs: ev.durationMs,
    exitCode: ev.exitCode,
    running: ev.phase === "started",
    stdout: ev.stdout,
    stderr: ev.stderr,
    stdoutBytes: ev.stdoutBytes,
    stderrBytes: ev.stderrBytes,
    stdoutTruncated: ev.stdoutTruncated,
    stderrTruncated: ev.stderrTruncated,
    outputOmitted: ev.outputOmitted,
    error: ev.error,
  };
}

function createGraphState(): GraphState {
  return shallowReactive<GraphState>({
    meta: null,
    pages: markRaw(new Map<number, GraphRow[]>()),
    pagesVersion: ref(0),
    inflight: markRaw(new Map<number, Promise<void>>()),
    loading: false,
    progress: 0,
  });
}

/** Drops every cached page and tells readers to re-read. */
export function clearPages(graph: GraphState): void {
  graph.pages.clear();
  graph.inflight.clear();
  graph.pagesVersion.value += 1;
}

export const useSessionsStore = defineStore("sessions", () => {
  const settings = useSettingsStore();
  const uiState = useUiStateStore();

  const sessions = shallowReactive(new Map<string, RepoSession>());
  const activeKey = ref<string | null>(null);
  /** Keys whose `repoOpen` call is still running, so the tree can show a spinner. */
  const opening = shallowReactive(new Set<string>());
  /** One-line error shown in the red strip under the toolbar. */
  const notice = ref<string | null>(null);
  /** Optional button shown next to the notice (for example "Pull…" after a push the remote refused). */
  const noticeAction = ref<NoticeAction | null>(null);
  /** One-line report that an action worked ("Committed …", "Stashed …"), shown in the status bar. */
  const successMessage = ref<string | null>(null);

  /**
   * Console buffers keyed by repoId. Probe commands are emitted during `repoOpen`, before the
   * session object exists, so a buffer is created on the first event and adopted by the session.
   */
  const consoleBuffers = new Map<string, ConsoleEntry[]>();

  function consoleBuffer(repoId: string): ConsoleEntry[] {
    let buffer = consoleBuffers.get(repoId);
    if (!buffer) {
      buffer = shallowReactive<ConsoleEntry[]>([]);
      consoleBuffers.set(repoId, buffer);
    }
    return buffer;
  }

  function trimBuffer(buffer: ConsoleEntry[]): void {
    const max = Math.max(1, settings.consoleMaxLines);
    if (buffer.length > max) buffer.splice(0, buffer.length - max);
  }

  function onConsoleEvent(ev: ConsoleEvent): void {
    const buffer = consoleBuffer(ev.repoId);
    // The matching `started` entry is almost always near the end.
    let idx = -1;
    for (let i = buffer.length - 1; i >= 0 && i >= buffer.length - 256; i -= 1) {
      if (buffer[i]?.seq === ev.seq) {
        idx = i;
        break;
      }
    }
    if (ev.phase === "started") {
      if (idx < 0) buffer.push(toEntry(ev));
    } else if (idx >= 0) {
      buffer[idx] = toEntry(ev);
    } else {
      buffer.push(toEntry(ev));
    }
    trimBuffer(buffer);
  }

  function onOperationProgress(ev: OperationProgress): void {
    const session = sessions.get(ev.repoId);
    const op = session?.operation;
    if (!session || !op || op.opId !== ev.opId) return;
    session.operation = { ...op, line: ev.line, cancellable: ev.cancellable };
  }

  // ---- Graph snapshot handling -------------------------------------------------------------

  function applyMeta(session: RepoSession, meta: GraphMeta): void {
    session.graph.meta = meta;
    session.graph.loading = meta.version === 0 || !meta.complete;
    if (meta.complete) session.graph.progress = meta.total;
  }

  /** Fetches the full `GraphMeta` (the snapshot event lacks head and worktree) once the version matches. */
  async function refreshMeta(session: RepoSession, version: number): Promise<void> {
    try {
      const meta = await unwrap(commands.graphMeta(session.key));
      if (session.graph.meta?.version === version && meta.version === version) applyMeta(session, meta);
    } catch {
      // The next snapshot event carries fresh numbers anyway.
    }
  }

  /**
   * Maps the previous first-visible row and the selection into the new snapshot, so the same row
   * stays under the cursor and the selection survives by oid.
   */
  async function reconcile(
    session: RepoSession,
    version: number,
    anchorOid: string | null,
    offset: number,
    selected: string[],
  ): Promise<void> {
    const oids = anchorOid !== null ? [anchorOid, ...selected] : [...selected];
    if (oids.length === 0) return;
    let found: (number | null)[];
    try {
      found = await unwrap(commands.graphFind(session.key, version, oids));
    } catch {
      // Stale version: a newer snapshot event has taken over.
      return;
    }
    if (session.graph.meta?.version !== version) return;

    let k = 0;
    if (anchorOid !== null) {
      const idx = found[k];
      k += 1;
      if (idx !== null && idx !== undefined) session.scrollTop = idx * ROW_HEIGHT + offset;
    }
    const keep = new Set<string>();
    for (const oid of selected) {
      const idx = found[k];
      k += 1;
      if (idx !== null && idx !== undefined) keep.add(oid);
    }
    session.selection = {
      oids: keep,
      anchorOid: session.selection.anchorOid !== null && keep.has(session.selection.anchorOid) ? session.selection.anchorOid : null,
    };
    if (session.selectedRow && !keep.has(session.selectedRow.oid)) {
      session.selectedRow = null;
      session.selectedFile = null;
      session.fileSelection = emptyFileSelection();
    }
  }

  function onGraphSnapshot(ev: GraphSnapshotEvent): void {
    const session = sessions.get(ev.repoId);
    if (!session) return;
    const previous = session.graph.meta;
    if (previous && ev.version < previous.version) return;

    const firstIndex = Math.floor(session.scrollTop / ROW_HEIGHT);
    const offset = session.scrollTop - firstIndex * ROW_HEIGHT;
    // Scrolled to the very top, the list stays at the top: rows that appear above the old first
    // row (the Working Tree row once status arrives after the cached paint, new commits) are
    // then in view instead of hidden above it. Anywhere else, the old first row stays put.
    const atTop = session.scrollTop < 1;
    const anchorOid = atTop ? null : (rowAt(session.graph, firstIndex)?.oid ?? null);
    const selected = [...session.selection.oids];

    applyMeta(session, {
      version: ev.version,
      total: ev.total,
      complete: ev.complete,
      truncated: ev.truncated,
      maxLanes: ev.maxLanes,
      openLanes: previous?.openLanes ?? 0,
      head: previous?.head ?? null,
      headRef: previous?.headRef ?? null,
      worktree: previous?.worktree ?? null,
      refColours: previous?.refColours ?? {},
      widestAuthors: previous?.widestAuthors ?? [],
    });
    clearPages(session.graph);
    void refreshMeta(session, ev.version);
    void reconcile(session, ev.version, anchorOid, offset, selected);
    void restoreLastSelected(session, ev.version, ev.complete);
  }

  /**
   * Selects the commit that was selected when the repository was last open, once a snapshot
   * holds it, and scrolls it into view. A large repository's first snapshot may be only its top
   * rows, so an incomplete snapshot that lacks the commit leaves the restore for the next one; a
   * complete one that lacks it ends it. Anything the user selects first wins.
   */
  async function restoreLastSelected(session: RepoSession, version: number, complete: boolean): Promise<void> {
    const oid = session.restoreOid;
    if (oid === null) return;
    const settled = () => session.restoreOid !== oid || session.graph.meta?.version !== version;
    if (session.selection.oids.size > 0) {
      session.restoreOid = null;
      return;
    }
    let row: GraphRow | undefined;
    try {
      const [index] = await unwrap(commands.graphFind(session.key, version, [oid]));
      if (settled()) return;
      if (index === null || index === undefined) {
        if (complete) session.restoreOid = null;
        return;
      }
      row = (await unwrap(commands.graphRows(session.key, version, index, 1)))[0];
    } catch {
      // A newer snapshot has taken over; its own event tries again.
      return;
    }
    if (settled() || !row) return;
    session.restoreOid = null;
    if (session.selection.oids.size > 0) return;
    selectRow(session, row);
    // Set the offset rather than `reveal`: a tab in the background has no height to scroll in.
    // A row on the first screen leaves the list at the top; any other lands a few rows down.
    if (row.index >= 10) session.scrollTop = (row.index - 3) * ROW_HEIGHT;
  }

  function onGraphProgress(ev: GraphProgressEvent): void {
    const session = sessions.get(ev.repoId);
    if (!session) return;
    session.graph.progress = ev.loaded;
    session.graph.loading = true;
  }

  /**
   * Watcher states that arrived before their session existed: `repo_open` starts the watcher and
   * announces it before it returns, so the first state always comes early. The session takes it
   * up when `openTab` creates it.
   */
  const earlyWatcherStates = new Map<string, WatcherStateEvent>();

  /**
   * A change in an SSH connection, which every repository on that server shares. A user left
   * out of the repository's connection settings comes from the SSH config, so then the host and
   * port decide.
   */
  function onTransportState(ev: TransportState): void {
    for (const session of sessions.values()) {
      const t = session.info.transport;
      if (t.type !== "ssh" || t.host !== ev.host || (t.port ?? 22) !== ev.port || (t.user !== null && t.user !== ev.user)) continue;
      const wasDown = session.connection !== null && session.connection.state !== "ready";
      session.connection = {
        state: ev.state,
        detail: ev.detail,
        retryAt: ev.retryInMs === null ? null : Date.now() + ev.retryInMs,
      };
      // Back after a loss: a graph that never loaded (its load failed with the connection) loads now.
      if (ev.state === "ready" && wasDown && !session.graph.loading && (session.graph.meta?.version ?? 0) === 0) {
        void refreshSession(session.key);
      }
    }
  }

  /** The graph load started when the tab opened failed, so no snapshot is coming. */
  function onGraphLoadFailed(ev: GraphLoadFailedEvent): void {
    const session = sessions.get(ev.repoId);
    if (!session) return;
    session.graph.loading = false;
    notify(`The commit history of ${session.info.displayName} could not be loaded: ${errorMessage(new IpcError(ev.error))}`, {
      label: "Try again",
      run: () => refreshSession(ev.repoId),
    });
  }

  function onWatcherState(ev: WatcherStateEvent): void {
    const session = sessions.get(ev.repoId);
    if (!session) {
      earlyWatcherStates.set(ev.repoId, ev);
      return;
    }
    session.watcherKind = ev.kind;
    session.watcherDetail = ev.detail;
  }

  // ---- Activity: how much each repository is looked at, which sets how often polling checks it.

  /** The activity last sent for each repository, so only changes reach the backend. */
  const sentActivity = new Map<string, Activity>();

  function activityOf(key: string): Activity {
    if (key !== activeKey.value || document.visibilityState === "hidden") return "hidden";
    return document.hasFocus() ? "focused" : "background";
  }

  function reportActivity(): void {
    for (const key of sessions.keys()) {
      const activity = activityOf(key);
      if (sentActivity.get(key) === activity) continue;
      sentActivity.set(key, activity);
      // Only sets how often polling checks; a failure leaves it at the last level.
      void commands.setRepoActivity(key, activity);
    }
    for (const key of sentActivity.keys()) if (!sessions.has(key)) sentActivity.delete(key);
  }

  watch([activeKey, () => sessions.size], reportActivity);
  window.addEventListener("focus", reportActivity);
  window.addEventListener("blur", reportActivity);
  document.addEventListener("visibilitychange", reportActivity);

  async function loadStatus(session: RepoSession): Promise<void> {
    session.status = await unwrap(commands.getRepoStatus(session.key));
    if (session.status.conflictedCount === 0) session.stashConflict = null;
    // A stash push or pop changes the count in status; a watcher event may not report it as a
    // refs change, so a count that no longer matches the list reloads the list too.
    if (session.stashes !== null && session.stashes.length !== session.status.stashCount) await loadStashes(session);
  }

  /** Refs and the stash list load together: every stash is an entry in the `refs/stash` reflog. */
  async function loadRefs(session: RepoSession): Promise<void> {
    await Promise.all([
      unwrap(commands.getRepoRefs(session.key)).then((refs) => (session.refs = refs)),
      loadStashes(session),
    ]);
  }

  async function loadStashes(session: RepoSession): Promise<void> {
    session.stashes = await unwrap(commands.getRepoStashes(session.key));
  }

  function onRepoChanged(ev: RepoChangedEvent): void {
    const session = sessions.get(ev.repoId);
    if (!session) return;
    // The backend already rebuilt the graph snapshot and emitted `graph-snapshot-event`.
    const tasks: Promise<void>[] = [];
    if (ev.status) tasks.push(loadStatus(session));
    if (ev.refs) tasks.push(loadRefs(session));
    void Promise.allSettled(tasks).then((results) => {
      for (const r of results) if (r.status === "rejected") notify(errorMessage(r.reason));
    });
  }

  let initialised: Promise<void> | null = null;

  /** Installs every global event listener exactly once. Safe to call more than once. */
  function init(): Promise<void> {
    if (!initialised) {
      initialised = Promise.all([
        events.consoleEvent.listen((event) => onConsoleEvent(event.payload)),
        events.graphSnapshotEvent.listen((event) => onGraphSnapshot(event.payload)),
        events.graphProgressEvent.listen((event) => onGraphProgress(event.payload)),
        events.repoChangedEvent.listen((event) => onRepoChanged(event.payload)),
        events.watcherStateEvent.listen((event) => onWatcherState(event.payload)),
        events.transportState.listen((event) => onTransportState(event.payload)),
        events.graphLoadFailedEvent.listen((event) => onGraphLoadFailed(event.payload)),
        events.operationProgress.listen((event) => onOperationProgress(event.payload)),
      ]).then(() => undefined);
    }
    return initialised;
  }

  const list = computed<RepoSession[]>(() => [...sessions.values()]);
  const active = computed<RepoSession | null>(() =>
    activeKey.value !== null ? (sessions.get(activeKey.value) ?? null) : null,
  );

  // A success message describes an action in one repository, so it goes when another tab is shown.
  watch(activeKey, () => {
    successMessage.value = null;
  });

  function setActive(key: string | null): void {
    if (key !== null && !sessions.has(key)) return;
    activeKey.value = key;
  }

  /** Starts the graph load and fetches status and refs, all in parallel. Failures become notices. */
  function bootstrap(session: RepoSession): void {
    session.graph.loading = true;
    const graph = unwrap(commands.graphOpen(session.key)).then((meta) => {
      if (session.graph.meta === null || meta.version >= session.graph.meta.version) applyMeta(session, meta);
      // A snapshot the backend already had comes with no snapshot event.
      if (meta.version > 0) void restoreLastSelected(session, meta.version, meta.complete);
    });
    void Promise.allSettled([graph, loadStatus(session), loadRefs(session)]).then((results) => {
      for (const r of results) if (r.status === "rejected") notify(errorMessage(r.reason));
    });
  }

  /** Opens a tab. Throws when `repoOpen` fails and creates no tab, so callers decide how to show it. */
  async function openTab(key: string): Promise<RepoSession> {
    const existing = sessions.get(key);
    if (existing) {
      setActive(key);
      return existing;
    }
    opening.add(key);
    try {
      const info = await unwrap(commands.repoOpen(key));
      const session: RepoSession = shallowReactive<RepoSession>({
        key,
        info,
        consoleEntries: consoleBuffer(key),
        graph: createGraphState(),
        status: null,
        refs: null,
        stashes: null,
        selection: { oids: new Set<string>(), anchorOid: null },
        selectedRow: null,
        selectedFile: null,
        fileSelection: emptyFileSelection(),
        commitDraft: "",
        // repo_open already gave the backend this list from ui-state.json.
        hiddenRefs: new Set(uiState.state?.perRepo?.[key]?.hiddenRefs ?? []),
        scrollTop: 0,
        reveal: null,
        restoreOid: uiState.state?.perRepo?.[key]?.lastSelectedOid ?? null,
        findOpen: false,
        findQuery: "",
        findFocus: 0,
        watcherKind: earlyWatcherStates.get(key)?.kind ?? null,
        watcherDetail: earlyWatcherStates.get(key)?.detail ?? null,
        connection: null,
        lastFetchAt: Date.now(),
        autoFetchFailed: false,
        stashConflict: null,
        operation: null,
        conflictComparison: DEFAULT_COMPARISON,
      });
      sessions.set(key, session);
      earlyWatcherStates.delete(key);
      setActive(key);
      bootstrap(session);
      return session;
    } finally {
      opening.delete(key);
    }
  }

  async function closeTab(key: string): Promise<void> {
    const keys = [...sessions.keys()];
    const idx = keys.indexOf(key);
    if (idx < 0) return;
    sessions.delete(key);
    consoleBuffers.delete(key);
    if (activeKey.value === key) {
      activeKey.value = keys[idx + 1] ?? keys[idx - 1] ?? null;
    }
    try {
      await unwrap(commands.repoClose(key));
    } catch (e) {
      notify(errorMessage(e));
    }
  }

  /** Full graph refresh plus fresh status and refs for one session (the toolbar Refresh button). */
  async function refreshSession(key: string): Promise<void> {
    const session = sessions.get(key);
    if (!session) return;
    session.graph.loading = true;
    const graph = unwrap(commands.graphRefresh(session.key, true)).then((meta) => {
      // The snapshot event usually lands first; only move forward, never back.
      if (session.graph.meta === null || meta.version > session.graph.meta.version) {
        applyMeta(session, meta);
        clearPages(session.graph);
      } else if (meta.version === session.graph.meta.version) {
        applyMeta(session, meta);
      }
    });
    const results = await Promise.allSettled([graph, loadStatus(session), loadRefs(session)]);
    // A failed graph load sends no snapshot, so nothing else would end the loading state.
    if (results[0].status === "rejected") session.graph.loading = false;
    for (const r of results) if (r.status === "rejected") notify(errorMessage(r.reason));
  }

  /**
   * Refetches refs and status after a write operation (checkout, merge, branch or tag). The
   * backend rebuilds the graph and emits `graph-snapshot-event` afterwards, but it does not emit
   * `repo-changed-event`, so without this call the Branches panel and the dirty-tree counts
   * would stay stale until the watcher noticed.
   */
  async function refetchRefsAndStatus(key: string): Promise<void> {
    const session = sessions.get(key);
    if (!session) return;
    const results = await Promise.allSettled([loadStatus(session), loadRefs(session)]);
    for (const r of results) if (r.status === "rejected") notify(errorMessage(r.reason));
  }

  /**
   * Switches a ref on or off in the commit graph. The backend rebuilds the graph from the commits
   * it already has and emits a new snapshot; the list is saved per repository in ui-state.json.
   */
  async function toggleRefVisible(key: string, fullName: string): Promise<void> {
    const session = sessions.get(key);
    if (!session) return;
    const next = new Set(session.hiddenRefs);
    if (next.has(fullName)) next.delete(fullName);
    else next.add(fullName);
    session.hiddenRefs = next;
    const list = [...next].sort();
    uiState.patchDebounced({ perRepo: { [key]: { hiddenRefs: list } } });
    try {
      const meta = await unwrap(commands.graphSetHiddenRefs(key, list));
      if (session.graph.meta === null || meta.version >= session.graph.meta.version) applyMeta(session, meta);
    } catch (e) {
      notify(errorMessage(e));
    }
  }

  /** Replaces the selection with one row and makes it the Files panel's subject. */
  function selectRow(session: RepoSession, row: GraphRow): void {
    session.selection = { oids: new Set([row.oid]), anchorOid: row.oid };
    session.selectedRow = row;
    session.selectedFile = null;
    session.fileSelection = emptyFileSelection();
    if (row.kind === "commit") uiState.patchDebounced({ perRepo: { [session.key]: { lastSelectedOid: row.oid } } });
  }

  /**
   * Selects the commit a ref points at and asks the graph view to scroll to it.
   * Returns false when the commit is not in the current snapshot.
   */
  async function selectCommit(key: string, oid: string): Promise<boolean> {
    const session = sessions.get(key);
    const meta = session?.graph.meta;
    if (!session || !meta || meta.version === 0) return false;
    try {
      const [index] = await unwrap(commands.graphFind(key, meta.version, [oid]));
      if (index === null || index === undefined) return false;
      const rows = await unwrap(commands.graphRows(key, meta.version, index, 1));
      const row = rows[0];
      if (!row) return false;
      selectRow(session, row);
      session.reveal = { index };
      return true;
    } catch (e) {
      if (!(e instanceof IpcError && e.error.kind === "staleVersion")) notify(errorMessage(e));
      return false;
    }
  }

  /**
   * Selects the row at `index` in snapshot `version` and asks the graph view to scroll to it (the
   * Find box). Returns false when that snapshot has been replaced or has no such row.
   */
  async function selectIndex(key: string, version: number, index: number): Promise<boolean> {
    const session = sessions.get(key);
    if (!session) return false;
    try {
      const row = (await unwrap(commands.graphRows(key, version, index, 1)))[0];
      if (!row || session.graph.meta?.version !== version) return false;
      selectRow(session, row);
      session.reveal = { index };
      return true;
    } catch (e) {
      if (!(e instanceof IpcError && e.error.kind === "staleVersion")) notify(errorMessage(e));
      return false;
    }
  }

  /** Shows the Find box over the active tab's Commits panel and puts the keyboard focus in it. */
  function openFind(): void {
    const session = active.value;
    if (!session) {
      notify("Open a repository first.");
      return;
    }
    session.findOpen = true;
    session.findFocus += 1;
  }

  /** Shows an error. It replaces the success message, which no longer describes the latest action. */
  function notify(message: string, action: NoticeAction | null = null): void {
    notice.value = message;
    noticeAction.value = action;
    successMessage.value = null;
  }

  /** Reports that an action worked. It clears an older error notice, which the action has moved past. */
  function notifySuccess(message: string): void {
    successMessage.value = message;
    dismissNotice();
  }

  function dismissNotice(): void {
    notice.value = null;
    noticeAction.value = null;
  }

  function dismissSuccess(): void {
    successMessage.value = null;
  }

  let restoring = false;

  /** Re-opens the tabs saved in `uiState.session`, one at a time, ignoring failures. */
  async function restore(): Promise<void> {
    const saved = uiState.state?.session;
    const tabs = saved?.openTabs ?? [];
    restoring = true;
    try {
      for (const key of tabs) {
        try {
          await openTab(key);
        } catch {
          // A repository that no longer opens simply does not get a tab.
        }
      }
      const wanted = saved?.activeTab ?? null;
      if (wanted !== null && sessions.has(wanted)) {
        activeKey.value = wanted;
      } else if (activeKey.value === null || !sessions.has(activeKey.value)) {
        activeKey.value = [...sessions.keys()][0] ?? null;
      }
    } finally {
      restoring = false;
    }
    writeSession();
  }

  function writeSession(): void {
    uiState.patchDebounced({
      session: { openTabs: [...sessions.keys()], activeTab: activeKey.value },
    });
  }

  watch(
    () => JSON.stringify([[...sessions.keys()], activeKey.value]),
    () => {
      if (!restoring) writeSession();
    },
  );

  return {
    sessions,
    list,
    activeKey,
    active,
    opening,
    notice,
    notifySuccess,
    noticeAction,
    successMessage,
    init,
    openTab,
    closeTab,
    setActive,
    restore,
    refreshSession,
    refetchRefsAndStatus,
    selectRow,
    selectCommit,
    selectIndex,
    openFind,
    toggleRefVisible,
    notify,
    dismissNotice,
    dismissSuccess,
    consoleBuffer,
  };
});
