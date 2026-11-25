import { commands, type GraphRow } from "@/lib/bindings";
import { IpcError, unwrap } from "@/lib/ipc";
import { pagesToDrop } from "@/lib/graph/pageWindow";
import type { GraphState, RepoSession } from "@/stores/sessions";

export const PAGE_SIZE = 200;
/** Pages fetched on either side of the visible window. */
const PREFETCH_PAGES = 2;
/** Pages kept beyond the prefetched ones before the furthest are dropped (see `dropFarPages`). */
const KEEP_PAGES = 3;

export function pageOf(index: number): number {
  return Math.floor(index / PAGE_SIZE);
}

/** The cached row at `index`, or `undefined` while its page has not arrived. Pure lookup. */
export function rowAt(graph: GraphState, index: number): GraphRow | undefined {
  if (index < 0) return undefined;
  const page = graph.pages.get(pageOf(index));
  return page ? page[index - pageOf(index) * PAGE_SIZE] : undefined;
}

/** Index of the cached row with this oid, searching loaded pages only. */
export function findLoadedIndex(graph: GraphState, oid: string): number | null {
  for (const [page, rows] of graph.pages) {
    for (let i = 0; i < rows.length; i += 1) {
      if ((rows[i] as GraphRow).oid === oid) return page * PAGE_SIZE + i;
    }
  }
  return null;
}

/**
 * Page cache on top of `graph_rows` for one session. Pages are keyed by number, fetched once,
 * dropped wholesale when the snapshot version changes (the store does that), and dropped one by
 * one when the visible window moves far from them (`dropFarPages`).
 *
 * `session` is asked for the session on every call, like a prop, and never kept: a component
 * that held on to the session it was created with would go on showing that one's rows if the
 * store ever put another session under the same key, while everything else showed the new one.
 */
export function useGraphPages(session: () => RepoSession) {
  async function loadPage(graph: GraphState, page: number, version: number): Promise<void> {
    const start = page * PAGE_SIZE;
    const key = session().key;
    try {
      const rows = await unwrap(commands.graphRows(key, version, start, PAGE_SIZE));
      // A snapshot swap while the request was out makes this page worthless.
      if (graph.meta?.version !== version) return;
      graph.pages.set(page, rows);
      graph.pagesVersion.value += 1;
    } catch (e) {
      if (!(e instanceof IpcError && e.error.kind === "staleVersion")) return;
      // Someone else knows a newer version than we do; catch up. The snapshot event that
      // follows (or already fired) clears the pages and the visible window refetches.
      try {
        const meta = await unwrap(commands.graphMeta(key));
        if (graph.meta === null || meta.version > graph.meta.version) {
          graph.meta = meta;
          graph.pages.clear();
          graph.pagesVersion.value += 1;
        }
      } catch {
        // Give up quietly; the next snapshot event fixes it.
      }
    }
  }

  function fetchPage(graph: GraphState, page: number, version: number): Promise<void> {
    const existing = graph.inflight.get(page);
    if (existing) return existing;
    const task: Promise<void> = loadPage(graph, page, version).finally(() => {
      if (graph.inflight.get(page) === task) graph.inflight.delete(page);
    });
    graph.inflight.set(page, task);
    return task;
  }

  /**
   * Makes sure the pages covering rows `start..end` (exclusive) plus two pages either side are
   * loaded or loading. Resolves when the pages for the exact range have arrived.
   */
  function ensureRange(start: number, end: number): Promise<void> {
    const graph = session().graph;
    const meta = graph.meta;
    if (!meta || meta.version === 0 || meta.total === 0) return Promise.resolve();
    const lastPage = pageOf(Math.max(0, meta.total - 1));
    const firstNeeded = Math.max(0, pageOf(Math.max(0, start)));
    const lastNeeded = Math.min(lastPage, pageOf(Math.max(0, end - 1)));
    const waits: Promise<void>[] = [];
    for (let p = firstNeeded; p <= lastNeeded; p += 1) {
      if (!graph.pages.has(p)) waits.push(fetchPage(graph, p, meta.version));
    }
    for (let p = Math.max(0, firstNeeded - PREFETCH_PAGES); p <= Math.min(lastPage, lastNeeded + PREFETCH_PAGES); p += 1) {
      if (!graph.pages.has(p) && !graph.inflight.has(p)) void fetchPage(graph, p, meta.version);
    }
    return Promise.all(waits).then(() => undefined);
  }

  /**
   * Drops cached pages far from the rows `start..end` (exclusive) on screen: anything beyond the
   * prefetched pages and `KEEP_PAGES` more on either side. Called when the window moves, never
   * from `ensureRange`, so the pages a Shift-range selection has just fetched stay until it has
   * read them.
   */
  function dropFarPages(start: number, end: number): void {
    const graph = session().graph;
    const drop = pagesToDrop(graph.pages.keys(), pageOf(Math.max(0, start)), pageOf(Math.max(0, end - 1)), PREFETCH_PAGES + KEEP_PAGES);
    if (drop.length === 0) return;
    for (const page of drop) graph.pages.delete(page);
    graph.pagesVersion.value += 1;
  }

  /** True while the page holding `index` is being fetched. */
  function isLoading(index: number): boolean {
    return session().graph.inflight.has(pageOf(index));
  }

  return {
    rowAt: (index: number) => rowAt(session().graph, index),
    findLoadedIndex: (oid: string) => findLoadedIndex(session().graph, oid),
    ensureRange,
    dropFarPages,
    isLoading,
  };
}
