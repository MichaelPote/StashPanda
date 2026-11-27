/**
 * The diff viewer's side of the line matching worker (`pairLines.worker.ts`): sends it the
 * blocks of one diff that were put off and hands back their pairs.
 */
import type { LinePair, PairRequest, PairResponse, PendingBlock } from "@/lib/diff/pairLines";

let worker: Worker | null = null;
let nextId = 1;
/** The request the worker is working on, if any. */
let running: { id: number; resolve: (pairs: Map<number, LinePair[] | null> | null) => void } | null = null;

function start(): Worker {
  const w = new Worker(new URL("./pairLines.worker.ts", import.meta.url), { type: "module" });
  w.onmessage = (event: MessageEvent<PairResponse>) => {
    const current = running;
    if (!current || current.id !== event.data.id) return;
    running = null;
    current.resolve(new Map(event.data.pairs));
  };
  // A worker that fails to start or throws leaves the diff as it is, paired by position.
  w.onerror = () => {
    const current = running;
    running = null;
    worker = null;
    w.terminate();
    current?.resolve(null);
  };
  return w;
}

/**
 * Matches `blocks` in the worker and resolves with each block's pairs by its number: null for a
 * block to leave as it is. Resolves with null, and no pairs at all, when another request took
 * over before this one was answered or the worker failed.
 *
 * One request runs at a time. A new one ends the worker in the middle of the old one and
 * starts another: the old diff is no longer on screen, and its blocks would only hold up the
 * new diff's.
 */
export function matchBlocks(blocks: PendingBlock[]): Promise<Map<number, LinePair[] | null> | null> {
  if (running) {
    running.resolve(null);
    running = null;
    worker?.terminate();
    worker = null;
  }
  worker ??= start();
  const id = nextId++;
  return new Promise((resolve) => {
    running = { id, resolve };
    (worker as Worker).postMessage({ id, blocks } satisfies PairRequest);
  });
}
