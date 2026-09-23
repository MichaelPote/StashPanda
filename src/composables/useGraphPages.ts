import { commands, type GraphRow } from "@/lib/bindings";
import { IpcError, unwrap } from "@/lib/ipc";
import type { GraphState, RepoSession } from "@/stores/sessions";

export const PAGE_SIZE = 200;
/** Pages fetched on either side of the visible window. */
const PREFETCH_PAGES = 2;

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
 * and dropped wholesale when the snapshot version changes (the store does that).
 */
export function useGraphPages(session: RepoSession) {
  const graph = session.graph;

  async function loadPage(page: number, version: number): Promise<void> {
    const start = page * PAGE_SIZE;
    try {
      const rows = await unwrap(commands.graphRows(session.key, version, start, PAGE_SIZE));
      // A snapshot swap while the request was out makes this page worthless.
      if (graph.meta?.version !== version) return;
      graph.pages.set(page, rows);
      graph.pagesVersion.value += 1;
    } catch (e) {
      if (!(e instanceof IpcError && e.error.kind === "staleVersion")) return;
      // Someone else knows a newer version than we do; catch up. The snapshot event that
      // follows (or already fired) clears the pages and the visible window refetches.
      try {
        const meta = await unwrap(commands.graphMeta(session.key));
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

  function fetchPage(page: number, version: number): Promise<void> {
    const existing = graph.inflight.get(page);
    if (existing) return existing;
    const task: Promise<void> = loadPage(page, version).finally(() => {
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
    const meta = graph.meta;
    if (!meta || meta.version === 0 || meta.total === 0) return Promise.resolve();
    const lastPage = pageOf(Math.max(0, meta.total - 1));
    const firstNeeded = Math.max(0, pageOf(Math.max(0, start)));
    const lastNeeded = Math.min(lastPage, pageOf(Math.max(0, end - 1)));
    const waits: Promise<void>[] = [];
    for (let p = firstNeeded; p <= lastNeeded; p += 1) {
      if (!graph.pages.has(p)) waits.push(fetchPage(p, meta.version));
    }
    for (let p = Math.max(0, firstNeeded - PREFETCH_PAGES); p <= Math.min(lastPage, lastNeeded + PREFETCH_PAGES); p += 1) {
      if (!graph.pages.has(p) && !graph.inflight.has(p)) void fetchPage(p, meta.version);
    }
    return Promise.all(waits).then(() => undefined);
  }

  /** True while the page holding `index` is being fetched. */
  function isLoading(index: number): boolean {
    return graph.inflight.has(pageOf(index));
  }

  return {
    rowAt: (index: number) => rowAt(graph, index),
    findLoadedIndex: (oid: string) => findLoadedIndex(graph, oid),
    ensureRange,
    isLoading,
  };
}
