/**
 * Which removed line became which added line, inside one block of changed lines. Git lists a
 * block's removed lines and then its added lines; it does not say which old line each new one
 * came from. The changed-word highlights compare a removed line with the added line paired with
 * it, and the side-by-side view lines the two up.
 *
 * Matching compares every removed line of a block with every added one, so it is done in two
 * places. The diff viewer's own thread does what is quick (`pairQuickly`) and draws the diff at
 * once; the blocks it put off go to a worker (`pairLines.worker.ts`), which matches them in
 * full (`pairThoroughly`) while the window stays free, and the diff is laid out again when the
 * answer comes back.
 */

/** A removed line's index in its block and the added line's index it is paired with. */
export type LinePair = [oldIndex: number, newIndex: number];

/** Lines this similar or more are paired; less similar lines count as removed and added. */
const MIN_SIMILARITY = 0.4;

/** How much comparing one block may take: removed × added lines, and lookups (see `countLookups`). */
interface Limits {
  cells: number;
  lookups: number;
}

/**
 * On the diff viewer's own thread: a block past either limit is put off. Comparing two lines
 * looks up each distinct piece of the shorter one, so a few hundred long minified lines stay
 * under the cell limit and would still keep the window busy for seconds; hence the second limit.
 */
const QUICK: Limits = { cells: 40_000, lookups: 2_000_000 };
/**
 * In the worker: a block of up to 1,000 by 1,000 lines, and about a second of comparing. Past
 * that the block stays paired by position; its two tables alone would take 16 MB per million cells.
 */
const THOROUGH: Limits = { cells: 1_000_000, lookups: 60_000_000 };

/**
 * What all the blocks of one diff may spend together on the viewer's own thread, in lookups:
 * about a twentieth of a second. The limits above are for one block, and a diff can have
 * hundreds of blocks that each stay under them: 188 blocks of 150 changed lines took 3.3
 * seconds, with the window frozen. Once this is spent, the blocks that are left are put off.
 */
const MAX_QUICK_LOOKUPS_PER_DIFF = 2_000_000;

/** How many lookups the blocks of one diff may still spend on the viewer's own thread. */
export interface PairBudget {
  lookups: number;
}

/** A full budget, for one diff: pass the same one to `pairQuickly` for each of its blocks. */
export function pairBudget(): PairBudget {
  return { lookups: MAX_QUICK_LOOKUPS_PER_DIFF };
}

/** Pairs the first removed line with the first added line, and so on: what Git's order gives. */
export function pairByPosition(oldCount: number, newCount: number): LinePair[] {
  return Array.from({ length: Math.min(oldCount, newCount) }, (_, i): LinePair => [i, i]);
}

/** The pairs of two-character pieces in a line, ignoring its indentation, with their counts. */
function bigrams(line: string): Map<string, number> {
  const text = line.trim();
  const out = new Map<string, number>();
  for (let i = 0; i + 1 < text.length; i += 1) {
    const piece = text.slice(i, i + 2);
    out.set(piece, (out.get(piece) ?? 0) + 1);
  }
  return out;
}

/**
 * How alike two lines are, from 0 to 1: the share of two-character pieces they have in common
 * (the Dice coefficient), so a line that was re-indented or had a word changed still scores
 * high. A line of one character or none matches only the same text.
 */
function similarity(a: string, b: string, ga: Map<string, number>, gb: Map<string, number>, sizeA: number, sizeB: number): number {
  if (sizeA === 0 || sizeB === 0) return a.trim() === b.trim() ? 1 : 0;
  let common = 0;
  const [small, large] = ga.size <= gb.size ? [ga, gb] : [gb, ga];
  for (const [piece, count] of small) common += Math.min(count, large.get(piece) ?? 0);
  return (2 * common) / (sizeA + sizeB);
}

/**
 * True when two lines are surely at least `MIN_SIMILARITY` alike, found without counting their
 * pieces: they start and end with the same text, as a line with one word changed does. Each
 * two-character piece inside the shared start and inside the shared end is in both lines, which
 * gives the least their similarity can be. False means only that this cannot tell.
 */
function alikeAtTheEnds(a: string, b: string): boolean {
  const x = a.trim();
  const y = b.trim();
  if (x === y) return true;
  const shorter = Math.min(x.length, y.length);
  if (shorter < 2) return false;
  let start = 0;
  while (start < shorter && x.charCodeAt(start) === y.charCodeAt(start)) start += 1;
  let end = 0;
  while (end < shorter - start && x.charCodeAt(x.length - 1 - end) === y.charCodeAt(y.length - 1 - end)) end += 1;
  const common = Math.max(0, start - 1) + Math.max(0, end - 1);
  return (2 * common) / (x.length + y.length - 2) >= MIN_SIMILARITY;
}

/** How many two-character pieces a line has, counting repeats. */
function sizeOf(g: Map<string, number>): number {
  let total = 0;
  for (const count of g.values()) total += count;
  return total;
}

/**
 * True when a block has as many added lines as removed ones and each removed line is alike the
 * added line in the same position: a rename or a reformat applied to a run of lines. Pairing by
 * position is then the answer, and finding that out compares each line with one other line,
 * not with every one. Most pairs are settled by `alikeAtTheEnds`, with no pieces counted at all.
 */
function alikeInPosition(oldLines: string[], newLines: string[]): boolean {
  if (oldLines.length !== newLines.length) return false;
  for (let i = 0; i < oldLines.length; i += 1) {
    const a = oldLines[i] as string;
    const b = newLines[i] as string;
    if (alikeAtTheEnds(a, b)) continue;
    const ga = bigrams(a);
    const gb = bigrams(b);
    if (similarity(a, b, ga, gb, sizeOf(ga), sizeOf(gb)) < MIN_SIMILARITY) return false;
  }
  return true;
}

/** How many lookups comparing every removed line with every added one would take. */
function countLookups(oldGrams: Map<string, number>[], newGrams: Map<string, number>[]): number {
  let lookups = 0;
  for (const a of oldGrams) for (const b of newGrams) lookups += Math.min(a.size, b.size);
  return lookups;
}

/**
 * The pairing with the highest total similarity that keeps both sides in their order, where
 * only lines at least `MIN_SIMILARITY` alike can pair.
 */
function bestPairing(oldLines: string[], newLines: string[], oldGrams: Map<string, number>[], newGrams: Map<string, number>[]): LinePair[] {
  const n = oldLines.length;
  const m = newLines.length;
  const oldSizes = oldGrams.map(sizeOf);
  const newSizes = newGrams.map(sizeOf);

  // best[i][j]: the highest total for the first i removed and first j added lines.
  const width = m + 1;
  const best = new Float64Array((n + 1) * width);
  const score = new Float64Array(n * m);
  for (let i = 1; i <= n; i += 1) {
    for (let j = 1; j <= m; j += 1) {
      const s = similarity(
        oldLines[i - 1] as string,
        newLines[j - 1] as string,
        oldGrams[i - 1] as Map<string, number>,
        newGrams[j - 1] as Map<string, number>,
        oldSizes[i - 1] as number,
        newSizes[j - 1] as number,
      );
      score[(i - 1) * m + (j - 1)] = s;
      const skip = Math.max(best[(i - 1) * width + j] as number, best[i * width + j - 1] as number);
      const take = s >= MIN_SIMILARITY ? (best[(i - 1) * width + j - 1] as number) + s : -1;
      best[i * width + j] = Math.max(skip, take);
    }
  }

  // Walk back from the end to read the pairs off.
  const pairs: LinePair[] = [];
  let i = n;
  let j = m;
  while (i > 0 && j > 0) {
    const here = best[i * width + j] as number;
    const s = score[(i - 1) * m + (j - 1)] as number;
    if (s >= MIN_SIMILARITY && here === (best[(i - 1) * width + j - 1] as number) + s) {
      pairs.push([i - 1, j - 1]);
      i -= 1;
      j -= 1;
    } else if (here === (best[(i - 1) * width + j] as number)) {
      i -= 1;
    } else {
      j -= 1;
    }
  }
  return pairs.reverse();
}

/** A block's pairs from the viewer's own thread, and whether they are the final ones. */
export interface QuickPairing {
  pairs: LinePair[];
  /**
   * False when the pairs are only by position because matching the block was put off: it is
   * past the limits for one block, or the diff's budget is spent. The worker then matches it.
   */
  settled: boolean;
}

/**
 * Pairs one block on the diff viewer's own thread, as far as that is quick. `budget` is shared
 * by the blocks of one diff. A block whose lines are alike in position is settled without
 * spending any of it.
 */
export function pairQuickly(oldLines: string[], newLines: string[], budget: PairBudget): QuickPairing {
  const n = oldLines.length;
  const m = newLines.length;
  if (n === 0 || m === 0) return { pairs: [], settled: true };
  if (alikeInPosition(oldLines, newLines)) return { pairs: pairByPosition(n, m), settled: true };
  const putOff: QuickPairing = { pairs: pairByPosition(n, m), settled: false };
  if (n * m > QUICK.cells || budget.lookups <= 0) return putOff;

  const oldGrams = oldLines.map(bigrams);
  const newGrams = newLines.map(bigrams);
  const lookups = countLookups(oldGrams, newGrams);
  if (lookups > QUICK.lookups) return putOff;
  if (lookups > budget.lookups) {
    // What is left is not spent on a smaller block further down: until the worker answers, the
    // diff would be matched, then not, then matched again, depending on each block's size.
    budget.lookups = 0;
    return putOff;
  }
  budget.lookups -= lookups;
  return { pairs: bestPairing(oldLines, newLines, oldGrams, newGrams), settled: true };
}

/**
 * Pairs one block in full, however long it takes within the worker's limits. Null for a block
 * past those: it stays paired by position.
 */
export function pairThoroughly(oldLines: string[], newLines: string[]): LinePair[] | null {
  const n = oldLines.length;
  const m = newLines.length;
  if (n === 0 || m === 0) return [];
  if (alikeInPosition(oldLines, newLines)) return pairByPosition(n, m);
  if (n * m > THOROUGH.cells) return null;
  const oldGrams = oldLines.map(bigrams);
  const newGrams = newLines.map(bigrams);
  if (countLookups(oldGrams, newGrams) > THOROUGH.lookups) return null;
  return bestPairing(oldLines, newLines, oldGrams, newGrams);
}

/**
 * Pairs removed lines with added lines by how alike they are, as far as is quick; see
 * `pairQuickly`, whose pairs these are. For callers with one block and no worker to finish it.
 */
export function pairBySimilarity(oldLines: string[], newLines: string[], budget: PairBudget = pairBudget()): LinePair[] {
  return pairQuickly(oldLines, newLines, budget).pairs;
}

/** One block the viewer put off, for the worker: its number in the diff and its lines. */
export interface PendingBlock {
  block: number;
  old: string[];
  new: string[];
}

/** What the worker is asked: the blocks of one diff that were put off. */
export interface PairRequest {
  id: number;
  blocks: PendingBlock[];
}

/**
 * The worker's answer: for each block asked about, its pairs, or null to leave it paired by
 * position (the block is past the worker's limits too, or the worker ran out of time).
 */
export interface PairResponse {
  id: number;
  pairs: [block: number, pairs: LinePair[] | null][];
}
