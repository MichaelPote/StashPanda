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
 * Pairs removed lines with added lines by how alike they are, keeping both in their order: the
 * pairing with the highest total similarity, where only lines at least `MIN_SIMILARITY` alike
 * can pair. A very large block is paired by position instead, which is fast.
 */
export function pairBySimilarity(oldLines: string[], newLines: string[]): LinePair[] {
  const n = oldLines.length;
  const m = newLines.length;
  if (n === 0 || m === 0) return [];
  if (n * m > MAX_CELLS) return pairByPosition(n, m);

  const oldGrams = oldLines.map(bigrams);
  const newGrams = newLines.map(bigrams);
  const sizeOf = (g: Map<string, number>): number => {
    let total = 0;
    for (const count of g.values()) total += count;
    return total;
  };
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
