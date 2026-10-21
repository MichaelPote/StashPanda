/**
 * Automatic widths for the Author, Date and ID columns of the commit table: wide enough for the
 * widest value the column can show and for its heading, measured with the real fonts. The list is
 * virtualised, so the widths come from what the column can contain rather than from the rows on
 * screen, which would make the columns jump while scrolling.
 */
import { ref } from "vue";
import { rootFontSizePx } from "@/lib/graph/paths";

/** Bumped when web fonts finish loading, so widths measured with a fallback font are redone. */
export const fontsVersion = ref(0);
if (typeof document !== "undefined" && document.fonts) {
  void document.fonts.ready.then(() => (fontsVersion.value += 1));
  document.fonts.addEventListener("loadingdone", () => (fontsVersion.value += 1));
}

let context: CanvasRenderingContext2D | null = null;

function textWidth(text: string, font: string, letterSpacingPx = 0): number {
  context ??= document.createElement("canvas").getContext("2d");
  if (!context) return text.length * 8;
  context.font = font;
  return context.measureText(text).width + letterSpacingPx * text.length;
}

function fontVar(name: "--font-sans" | "--font-mono"): string {
  const value = getComputedStyle(document.documentElement).getPropertyValue(name).trim();
  return value || (name === "--font-mono" ? "monospace" : "sans-serif");
}

/** Cell padding `px-1.5` on both sides, plus 2px so rounding never truncates. */
function cellPadding(): number {
  return 2 * 0.375 * rootFontSizePx() + 2;
}

/** A heading in the `column-head` style: 10px, semibold, upper case, 0.06em spacing, `px-2`. */
function headingWidth(label: string): number {
  return textWidth(label.toUpperCase(), `600 10px ${fontVar("--font-sans")}`, 0.6) + 2 * 0.5 * rootFontSizePx() + 2;
}

function rowFont(): string {
  return `400 ${rootFontSizePx()}px ${fontVar("--font-sans")}`;
}

/** Widest of `texts` in the regular row font (`text-base`, IBM Plex Sans), in pixels. */
export function rowTextWidth(texts: Iterable<string>): number {
  const font = rowFont();
  let widest = 0;
  for (const t of texts) widest = Math.max(widest, textWidth(t, font));
  return widest;
}

export function autoAuthorWidth(names: readonly string[]): number {
  const font = rowFont();
  const widest = names.reduce((max, name) => Math.max(max, textWidth(name, font)), 0);
  // The avatar (size-4) and the gap after it (gap-1.5).
  const avatar = rootFontSizePx() + 0.375 * rootFontSizePx();
  return Math.ceil(Math.max(headingWidth("Author"), widest + avatar + cellPadding()));
}

/**
 * The Author column's width when it shows only the pictures (`appearance.authorAvatarOnly`): one
 * avatar (size-4) and the cell's padding, or the "Author" heading when the headings show and it is
 * wider.
 */
export function autoAvatarOnlyWidth(withHeading: boolean): number {
  const avatar = rootFontSizePx() + cellPadding();
  return Math.ceil(withHeading ? Math.max(headingWidth("Author"), avatar) : avatar);
}

/** The widest strings each date format produces: two-digit counts for every unit, or all zeros. */
const RELATIVE_SAMPLES = [
  "just now",
  "in a moment",
  "59 seconds ago",
  "59 minutes ago",
  "23 hours ago",
  "6 days ago",
  "4 weeks ago",
  "11 months ago",
  "99 years ago",
];

export function autoDateWidth(format: "relative" | "absolute"): number {
  const font = rowFont();
  const samples = format === "absolute" ? ["0000-00-00 00:00"] : RELATIVE_SAMPLES;
  const widest = samples.reduce((max, s) => Math.max(max, textWidth(s, font)), 0);
  return Math.ceil(Math.max(headingWidth("Date"), widest + cellPadding()));
}

/** Seven characters of the monospace font at `text-xs`. */
export function autoIdWidth(): number {
  const widest = textWidth("0000000", `400 ${0.75 * rootFontSizePx()}px ${fontVar("--font-mono")}`);
  return Math.ceil(Math.max(headingWidth("ID"), widest + cellPadding()));
}

/** The commit table's columns that have a width of their own; the commit message column takes the rest. */
export type SizedColumn = "graph" | "author" | "date" | "id";

/** The narrowest and widest a drag can make each column. */
export const COLUMN_LIMITS: Record<SizedColumn, { min: number; max: number }> = {
  graph: { min: 28, max: 4000 },
  // At its narrowest the Author column shows only the avatars: 14px plus 5.25px of padding each side.
  author: { min: 25, max: 400 },
  date: { min: 56, max: 260 },
  id: { min: 40, max: 400 },
};

/**
 * The widths to draw the columns at, so that the commit message column keeps at least
 * `messageMin` of the `room` the table has. The widths saved from a drag can add up to more than
 * that: dragged wide in a wider window, or the window made narrower since. When they do, every
 * column gives up the same share of what it has above its own minimum, so none goes below it.
 * The saved widths are left as they are, so the columns grow back when there is room again.
 * `room` of 0 (not measured yet) leaves the widths as given.
 */
export function fitColumns(widths: Record<SizedColumn, number>, room: number, messageMin: number): Record<SizedColumn, number> {
  const columns = Object.keys(widths) as SizedColumn[];
  const total = columns.reduce((sum, c) => sum + widths[c], 0);
  const excess = total - Math.max(0, room - messageMin);
  if (room <= 0 || excess <= 0) return widths;
  const spare = columns.reduce((sum, c) => sum + Math.max(0, widths[c] - COLUMN_LIMITS[c].min), 0);
  const share = spare > 0 ? Math.min(1, excess / spare) : 0;
  const fitted = { ...widths };
  for (const c of columns) {
    const above = Math.max(0, widths[c] - COLUMN_LIMITS[c].min);
    fitted[c] = Math.floor(widths[c] - above * share);
  }
  return fitted;
}
