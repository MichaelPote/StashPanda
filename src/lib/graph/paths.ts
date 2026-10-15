import type { GraphRow } from "@/lib/bindings";

/** Height of one commit row in pixels. Shared by the virtual list, the SVG and the store. */
/** Root font size in CSS pixels (14px, set on <html> in index.css). */
export function rootFontSizePx(): number {
  if (typeof document === "undefined") return 14;
  const v = parseFloat(getComputedStyle(document.documentElement).fontSize);
  return Number.isFinite(v) && v > 0 ? v : 14;
}
export function remToPx(rem: number): number {
  return Math.round(rem * rootFontSizePx());
}
/**
 * Commit row height in whole pixels, for the virtual list and the SVG. It must equal `--row-h` in
 * index.css, which the rows are drawn with.
 */
export const ROW_HEIGHT = 22;
/** Horizontal distance between two lane centres in pixels. */
export const LANE_WIDTH = 14;
/** Number of `--graph-lane-N` CSS variables. */
export const PALETTE_SIZE = 12;


/** Palette index actually used for a lane colour (the backend already wraps, this is a guard). */
export function paletteIndex(colour: number): number {
  return ((colour % PALETTE_SIZE) + PALETTE_SIZE) % PALETTE_SIZE;
}

/** CSS colour expression for a palette index. */
export function laneColour(colour: number): string {
  return `var(--graph-lane-${paletteIndex(colour)})`;
}

/**
 * Inline style that gives a ref pill its lane's colour, strong tint, chip fill, subtle tint and
 * solid-pill text.
 */
export function laneVars(colour: number): Record<string, string> {
  const i = paletteIndex(colour);
  return {
    "--lane": `var(--graph-lane-${i})`,
    "--lane-strong": `var(--graph-lane-strong-${i})`,
    "--lane-bg": `var(--graph-lane-bg-${i})`,
    "--lane-subtle": `var(--graph-lane-subtle-${i})`,
    "--lane-ink": `var(--graph-lane-ink-${i})`,
  };
}

/** Centre x of a lane. */
export function laneX(lane: number, laneWidth: number = LANE_WIDTH): number {
  return lane * laneWidth + laneWidth / 2;
}

/** Default graph column width: enough for `maxLanes + 1` lanes, between 3 and 12 lanes wide. */
/**
 * Width the graph column needs for `maxLanes` lanes: every lane plus half a lane of breathing
 * room on the right. No upper bound: the column grows with the graph.
 */
export function autoGraphColumnWidth(maxLanes: number): number {
  const lanes = Math.max(1, maxLanes);
  return Math.round(lanes * LANE_WIDTH + LANE_WIDTH / 2);
}

export interface ColourPath {
  colour: number;
  d: string;
}

export interface LaneDot {
  /** Row index in the snapshot, used as the element key. */
  index: number;
  cx: number;
  cy: number;
  colour: number;
  isHead: boolean;
  isWorkingTree: boolean;
}

export interface LanePaths {
  /** Straight pass-through runs, one `<path>` per colour. */
  runs: ColourPath[];
  /** Cubic Bezier lane changes, one `<path>` per colour. */
  curves: ColourPath[];
  /** Fading stubs for lanes whose parent is outside the snapshot, one `<path>` per colour. */
  stubs: ColourPath[];
  dots: LaneDot[];
}

interface OpenRun {
  x: number;
  yStart: number;
  yEnd: number;
  lastRow: number;
}

function fmt(n: number): string {
  // Keep the path text short; half-pixel precision is plenty for 2 px strokes.
  return Number.isInteger(n) ? String(n) : n.toFixed(1);
}

/**
 * Builds the SVG paths for one visible window of rows.
 *
 * `rows[k]` is the row at snapshot index `first + k`, or `undefined` while its page is still
 * loading. Row k's centre is at `y = k * rowHeight + rowHeight / 2`; its edges run from that
 * centre to the centre of row k + 1, so an edge stored on the last row of the window reaches
 * one row past the SVG's height, which the caller clips.
 */
export function buildLanePaths(
  rows: readonly (GraphRow | undefined)[],
  first: number,
  rowHeight: number = ROW_HEIGHT,
  laneWidth: number = LANE_WIDTH,
): LanePaths {
  const H = rowHeight;
  const half = H / 2;
  const openRuns = new Map<string, OpenRun>();
  const runsByColour = new Map<number, string[]>();
  const curvesByColour = new Map<number, string[]>();
  const stubsByColour = new Map<number, string[]>();
  const dots: LaneDot[] = [];

  const flushRun = (key: string, run: OpenRun): void => {
    const colour = Number(key.slice(key.indexOf(":") + 1));
    let list = runsByColour.get(colour);
    if (!list) {
      list = [];
      runsByColour.set(colour, list);
    }
    list.push(`M${fmt(run.x)},${fmt(run.yStart)}V${fmt(run.yEnd)}`);
  };

  for (let k = 0; k < rows.length; k += 1) {
    const row = rows[k];
    if (!row) continue;
    const y0 = k * H + half;
    const y1 = y0 + H;

    for (const edge of row.edges) {
      const x0 = laneX(edge.fromLane, laneWidth);
      const x1 = laneX(edge.toLane, laneWidth);
      const colour = paletteIndex(edge.colour);

      if (edge.kind === "dangling") {
        let list = stubsByColour.get(colour);
        if (!list) {
          list = [];
          stubsByColour.set(colour, list);
        }
        list.push(`M${fmt(x0)},${fmt(y0)}V${fmt(y0 + H * 0.6)}`);
        continue;
      }

      if (edge.fromLane === edge.toLane) {
        const key = `${edge.fromLane}:${colour}`;
        const open = openRuns.get(key);
        if (open && open.lastRow === k - 1) {
          open.yEnd = y1;
          open.lastRow = k;
        } else {
          if (open) flushRun(key, open);
          openRuns.set(key, { x: x0, yStart: y0, yEnd: y1, lastRow: k });
        }
        continue;
      }

      let list = curvesByColour.get(colour);
      if (!list) {
        list = [];
        curvesByColour.set(colour, list);
      }
      const ym = fmt(y0 + half);
      list.push(`M${fmt(x0)},${fmt(y0)}C${fmt(x0)},${ym} ${fmt(x1)},${ym} ${fmt(x1)},${fmt(y1)}`);
    }

    dots.push({
      index: first + k,
      cx: laneX(row.lane, laneWidth),
      cy: y0,
      colour: paletteIndex(row.colour),
      isHead: row.isHead,
      isWorkingTree: row.kind === "workingTree",
    });
  }

  for (const [key, run] of openRuns) flushRun(key, run);

  const toPaths = (m: Map<number, string[]>): ColourPath[] =>
    [...m.entries()].map(([colour, parts]) => ({ colour, d: parts.join("") }));

  return {
    runs: toPaths(runsByColour),
    curves: toPaths(curvesByColour),
    stubs: toPaths(stubsByColour),
    dots,
  };
}
