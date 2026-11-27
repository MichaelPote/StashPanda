/**
 * Matches removed lines to added lines off the main thread, for the blocks of a diff that the
 * diff viewer put off (`pairQuickly` in `pairLines.ts`). It answers once, when every block is
 * done, so the diff is laid out again a single time.
 */
import { pairThoroughly, type PairRequest, type PairResponse } from "@/lib/diff/pairLines";

/**
 * How long one diff may take here. Blocks not reached by then stay paired by position. The
 * viewer ends this worker when another diff is shown meanwhile, so this only bounds how late
 * a very large diff can still shift under the reader.
 */
const MAX_MS = 8_000;

self.onmessage = (event: MessageEvent<PairRequest>) => {
  const { id, blocks } = event.data;
  const deadline = performance.now() + MAX_MS;
  const pairs: PairResponse["pairs"] = blocks.map((b) => [b.block, performance.now() > deadline ? null : pairThoroughly(b.old, b.new)]);
  self.postMessage({ id, pairs } satisfies PairResponse);
};
