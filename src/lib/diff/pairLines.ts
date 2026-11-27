/**
 * Which removed line became which added line, inside one block of changed lines. Git lists a
 * block's removed lines and then its added lines; it does not say which old line each new one
 * came from. The changed-word highlights compare a removed line with the added line paired with
 * it, and the side-by-side view lines the two up.
 */

/** A removed line's index in its block and the added line's index it is paired with. */
export type LinePair = [oldIndex: number, newIndex: number];

/** Lines this similar or more are paired; less similar lines count as removed and added. */
const MIN_SIMILARITY = 0.4;
/** Blocks larger than this many removed × added lines fall back to pairing by position. */
const MAX_CELLS = 40_000;
/**
 * Blocks whose comparisons would take more than this many lookups fall back too. Comparing two
 * lines looks up each distinct piece of the shorter one, so a few hundred long minified lines stay
 * under `MAX_CELLS` and would still keep the window busy for seconds.
 */
const MAX_LOOKUPS = 2_000_000;
/**
 * What all the blocks of one diff may spend together. The two limits above are for one block,
 * and a diff can have hundreds of blocks that each stay under them: 188 blocks of 150 changed
 * lines took 3.3 seconds, with the window frozen. This many lookups take between a tenth and a
 * fifth of a second; once they are spent, the blocks that are left are paired by position.
 */
const MAX_LOOKUPS_PER_DIFF = 6_000_000;

/** How many lookups the blocks of one diff may still spend on pairing by similarity. */
export interface PairBudget {
  lookups: number;
}

/** A full budget, for one diff: pass the same one to `pairBySimilarity` for each of its blocks. */
export function pairBudget(): PairBudget {
  return { lookups: MAX_LOOKUPS_PER_DIFF };
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

/**
 * Pairs removed lines with added lines by how alike they are, keeping both in their order: the
 * pairing with the highest total similarity, where only lines at least `MIN_SIMILARITY` alike
 * can pair. A very large block is paired by position instead, which is fast, and so is every
 * block once `budget`, shared by the blocks of one diff, is spent.
 */
export function pairBySimilarity(oldLines: string[], newLines: string[], budget: PairBudget = pairBudget()): LinePair[] {
  const n = oldLines.length;
  const m = newLines.length;
  if (n === 0 || m === 0) return [];
  if (n * m > MAX_CELLS || budget.lookups <= 0) return pairByPosition(n, m);
  if (alikeInPosition(oldLines, newLines)) return pairByPosition(n, m);

  const oldGrams = oldLines.map(bigrams);
  const newGrams = newLines.map(bigrams);
  let lookups = 0;
  for (const a of oldGrams) for (const b of newGrams) lookups += Math.min(a.size, b.size);
  if (lookups > MAX_LOOKUPS) return pairByPosition(n, m);
  if (lookups > budget.lookups) {
    // What is left is not spent on a smaller block further down: the diff would be matched,
    // then not, then matched again, depending on each block's size.
    budget.lookups = 0;
    return pairByPosition(n, m);
  }
  budget.lookups -= lookups;
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
