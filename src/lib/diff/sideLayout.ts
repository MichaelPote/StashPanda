/**
 * The side-by-side diff without filler rows: each side shows only its own lines, and the gap
 * between them draws each change as a curved band from its old lines to its new lines.
 *
 * Both sides scroll with one scrollbar, measured in "rows": a stretch of unchanged lines is as
 * many rows as it has lines, and a change is as many rows as its longer side. Inside a change
 * the shorter side moves more slowly, so the two sides line up again once it is past.
 */

/** Width of the gap between the two sides, where the curves are drawn. */
export const GAP_PX = 36;

/** One stretch of the diff, in the shared rows and in each side's own lines. */
export interface Band {
  /** First row of the stretch, and how many rows it takes. */
  y: number;
  h: number;
  /** First line on the left (old) side, and how many lines it has there. */
  l: number;
  lh: number;
  /** First line on the right (new) side, and how many lines it has there. */
  r: number;
  rh: number;
  /** "change" for deleted or added lines, "hunk" for a hunk header, "same" for lines on both sides. */
  kind: "change" | "hunk" | "same";
  /**
   * A change that is a whole block of Git's diff with no lines on one side: a pure insertion or
   * deletion, the only kind drawn with an outline and a line marking where its lines would go.
   * A block with lines on both sides, even when matching splits it into several bands, has no
   * borders inside it.
   */
  outline?: boolean;
}

/** Builds the bands as lines are added to each side, merging neighbouring stretches of "same". */
export class BandBuilder {
  readonly bands: Band[] = [];
  rows = 0;
  private left = 0;
  private right = 0;

  add(kind: Band["kind"], lh: number, rh: number, outline = false): void {
    if (lh === 0 && rh === 0) return;
    const h = Math.max(lh, rh);
    const last = this.bands[this.bands.length - 1];
    if (kind === "same" && last?.kind === "same") {
      last.h += h;
      last.lh += lh;
      last.rh += rh;
    } else {
      this.bands.push({ y: this.rows, h, l: this.left, lh, r: this.right, rh, kind, outline });
    }
    this.rows += h;
    this.left += lh;
    this.right += rh;
  }
}

/** The band holding `row`, by binary search; the last band for a row past the end. */
function bandAt(bands: Band[], row: number): Band | undefined {
  let lo = 0;
  let hi = bands.length - 1;
  while (lo < hi) {
    const mid = (lo + hi + 1) >> 1;
    if ((bands[mid] as Band).y <= row) lo = mid;
    else hi = mid - 1;
  }
  return bands[lo];
}

/**
 * The line on one side (a fraction, counted from 0) at shared row `row`. Inside a change it
 * moves in proportion: a 4-line side of a 13-row change moves 4/13 of a line per row.
 */
export function sideLine(bands: Band[], row: number, side: "left" | "right"): number {
  const b = bandAt(bands, row);
  if (!b) return 0;
  const start = side === "left" ? b.l : b.r;
  const count = side === "left" ? b.lh : b.rh;
  const into = Math.min(Math.max(row - b.y, 0), b.h);
  return start + (b.h > 0 ? (into * count) / b.h : 0);
}

/** Where each side's first line sits, in pixels from the top of the scrolled content. */
export interface SideOffsets {
  left: number;
  right: number;
}

/**
 * Each side's offset for a scroll position. The two sides are lined up at an anchor that moves
 * from the top of the view to its bottom as the view scrolls from the start to the end, so both
 * sides start at the top together and end at the bottom together.
 */
export function sideOffsets(bands: Band[], totalRows: number, rowPx: number, scrollTop: number, viewHeight: number): SideOffsets {
  const maxScroll = Math.max(0, totalRows * rowPx - viewHeight);
  const t = maxScroll > 0 ? Math.min(1, Math.max(0, scrollTop / maxScroll)) : 0;
  const anchorPx = t * Math.min(viewHeight, totalRows * rowPx);
  const row = (scrollTop + anchorPx) / rowPx;
  return {
    left: scrollTop + anchorPx - sideLine(bands, row, "left") * rowPx,
    right: scrollTop + anchorPx - sideLine(bands, row, "right") * rowPx,
  };
}

/**
 * The SVG path of one band across the gap, `width` wide: the left side's lines from `lt` to `lb`
 * joined to the right side's from `rt` to `rb` by curves that leave and arrive level. A side
 * with no lines has `lt === lb`, so the band narrows to a point there.
 */
export function bandPath(width: number, lt: number, lb: number, rt: number, rb: number): string {
  const c = width / 2;
  return `M0 ${lt}C${c} ${lt} ${c} ${rt} ${width} ${rt}L${width} ${rb}C${c} ${rb} ${c} ${lb} 0 ${lb}Z`;
}

/** The top and bottom curves of the same band, without its sides, for drawing its outline. */
export function bandEdges(width: number, lt: number, lb: number, rt: number, rb: number): string {
  const c = width / 2;
  return `M0 ${lt}C${c} ${lt} ${c} ${rt} ${width} ${rt}M0 ${lb}C${c} ${lb} ${c} ${rb} ${width} ${rb}`;
}

/**
 * The top and bottom of a band's outline on one side. They sit on the half pixel, so each 1px
 * edge fills one row of pixels instead of two half rows. A side with no lines comes to a point
 * on the half pixel too, where the line marking the insertion is drawn, so the two meet exactly.
 */
export function sideEdges(y: number, height: number): [number, number] {
  return height === 0 ? [y + 0.5, y + 0.5] : [y + 0.5, y + height - 0.5];
}

/**
 * The top and bottom of a band's fill on one side: exactly its lines, so neighbouring bands of
 * one block meet with no gap between them. An outlined band's point sits on the half pixel, on
 * the line marking where its lines would go.
 */
export function sideFill(y: number, height: number, outline: boolean): [number, number] {
  if (height === 0) return outline ? [y + 0.5, y + 0.5] : [y, y];
  return [y, y + height];
}
