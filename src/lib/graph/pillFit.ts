/**
 * How many of a commit row's ref pills fit beside its message: the rest go into a "+N" pill that
 * opens them in a popover. The widths are the pills' own, measured in the row.
 */

/**
 * The number of pills, from the first, that fit in `room` pixels with `gap` between each two,
 * leaving space for the "+N" pill (`moreWidth`) whenever any are left out. At least `min` show
 * even when they do not fit, clipped at the column's edge instead.
 */
export function pillsThatFit(widths: readonly number[], room: number, gap: number, moreWidth: number, min = 1): number {
  const n = widths.length;
  let used = 0;
  for (const w of widths) used += w;
  if (used + gap * Math.max(0, n - 1) <= room) return n;
  let fits = 0;
  used = 0;
  for (let k = 1; k < n; k += 1) {
    used += widths[k - 1] + gap;
    // k pills, each followed by a gap, then the "+N" pill.
    if (used + moreWidth > room) break;
    fits = k;
  }
  return Math.max(Math.min(min, n), fits);
}
