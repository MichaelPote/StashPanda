/**
 * Which cached pages of the commit graph to drop. A page is 200 rows, and in the middle of a
 * large history one page can hold thousands of lane edges (2.4 MB of JSON at row 40,000 of
 * git.git), so keeping every page fetched while scrolling grows without bound.
 */

/**
 * Pages to drop when the rows `firstPage..lastPage` (inclusive) are on screen: every cached page
 * more than `keep` pages away from them. Returned in the order `cached` lists them.
 */
export function pagesToDrop(cached: Iterable<number>, firstPage: number, lastPage: number, keep: number): number[] {
  const lo = firstPage - keep;
  const hi = lastPage + keep;
  const out: number[] = [];
  for (const page of cached) {
    if (page < lo || page > hi) out.push(page);
  }
  return out;
}
