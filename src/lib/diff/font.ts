/**
 * How the diff text is drawn, from the `diff` section of settings.json. `DiffViewer.vue` and the
 * Settings dialog's preview both style their rows with `diffTextStyle`, so the preview shows
 * exactly what the viewer will.
 */
import type { CSSProperties } from "vue";
import type { DiffSettings, FontSmoothing } from "@/lib/bindings";

/** Fonts bundled with the app from their @fontsource packages (imported in `main.ts`). */
export const BUNDLED_DIFF_FONTS = ["JetBrains Mono", "IBM Plex Mono"] as const;

export interface DiffFont {
  fontFamily: string;
  /** CSS pixels. */
  fontSize: number;
  /** Row height as a multiple of the font size. */
  lineHeight: number;
  tabWidth: number;
  ligatures: boolean;
  smoothing: FontSmoothing;
}

export const DIFF_FONT_DEFAULTS: DiffFont = {
  fontFamily: "JetBrains Mono",
  fontSize: 12,
  lineHeight: 1.2,
  tabWidth: 8,
  ligatures: true,
  smoothing: "clearType",
};

const SMOOTHINGS: readonly FontSmoothing[] = ["clearType", "grayscale", "none"];

function clamp(value: unknown, min: number, max: number, fallback: number): number {
  return typeof value === "number" && Number.isFinite(value) ? Math.min(max, Math.max(min, value)) : fallback;
}

/** The diff settings with defaults for anything missing and limits on hand-edited numbers. */
export function resolveDiffFont(diff: Partial<DiffSettings> | null | undefined): DiffFont {
  const d = DIFF_FONT_DEFAULTS;
  const family = typeof diff?.fontFamily === "string" ? diff.fontFamily.trim() : "";
  return {
    fontFamily: family.length > 0 ? family : d.fontFamily,
    fontSize: clamp(diff?.fontSize, 6, 48, d.fontSize),
    lineHeight: clamp(diff?.lineHeight, 1, 3, d.lineHeight),
    tabWidth: Math.round(clamp(diff?.tabWidth, 1, 16, d.tabWidth)),
    ligatures: typeof diff?.ligatures === "boolean" ? diff.ligatures : d.ligatures,
    smoothing: SMOOTHINGS.includes(diff?.fontSmoothing as FontSmoothing) ? (diff?.fontSmoothing as FontSmoothing) : d.smoothing,
  };
}

/** A font family name as a CSS value, with the app's monospace stack behind it. */
export function cssFontFamily(name: string): string {
  const quoted = `"${name.replace(/\\/g, "\\\\").replace(/"/g, '\\"')}"`;
  return `${quoted}, var(--font-mono)`;
}

/** Row height in whole pixels, so rows never drift from where the virtual list expects them. */
export function diffRowPx(font: DiffFont): number {
  return Math.max(1, Math.round(font.fontSize * font.lineHeight));
}

/**
 * How each `diff.fontSmoothing` value is drawn. Windows WebView2 (Chromium) ignores
 * `-webkit-font-smoothing`, which only works on macOS; it is set as well for WebKit. Measured in
 * Edge 154 on Windows on 2026-10-01:
 * - ClearType is what Chromium draws by default, but only for text that is not on a compositor
 *   layer of its own. So nothing around the diff text may set `will-change: transform` (which
 *   the virtual list's row container used to), or the text silently turns grayscale and soft.
 * - Grayscale: `will-change: transform` on the element holding the rows puts them on their own
 *   layer, where Chromium never uses ClearType.
 * - None: Chromium has no per-element way to switch antialiasing off. Instead the
 *   `#diff-aliased-text` SVG filter in `index.html` rounds each pixel's coverage to fully on or
 *   fully off. It must sit on elements that hold only text over a transparent background, since
 *   it would round a translucent background (a row's colour, a changed-word highlight) away too:
 *   those are the `diff-glyphs` spans, styled in `index.css` through `--diff-glyph-filter`.
 */
function smoothingStyle(smoothing: FontSmoothing): Record<string, string> {
  switch (smoothing) {
    case "clearType":
      return { WebkitFontSmoothing: "subpixel-antialiased" };
    case "grayscale":
      return { WebkitFontSmoothing: "antialiased", willChange: "transform" };
    case "none":
      return { WebkitFontSmoothing: "none", "--diff-glyph-filter": "url(#diff-aliased-text)" };
  }
}

/**
 * Styles for an element holding diff rows. The gutter widths are in `ch` so that line numbers up
 * to 99999 fit at any font size; at 10.5px JetBrains Mono (the fixed size before the setting)
 * they match the old fixed 2.5rem and 0.75rem.
 */
export function diffTextStyle(font: DiffFont): CSSProperties {
  return {
    ...smoothingStyle(font.smoothing),
    fontFamily: cssFontFamily(font.fontFamily),
    fontSize: `${font.fontSize}px`,
    lineHeight: `${diffRowPx(font)}px`,
    tabSize: font.tabWidth,
    fontVariantLigatures: font.ligatures ? "normal" : "none",
    "--diff-row-h": `${diffRowPx(font)}px`,
    "--diff-num-w": "5.5ch",
    "--diff-mark-w": "1.7ch",
  } as CSSProperties;
}

/** Display width of a line in characters, with each tab advancing to the next multiple of `tabWidth`. */
export function displayLength(text: string, tabWidth: number): number {
  let col = 0;
  for (const ch of text) col += ch === "\t" ? tabWidth - (col % tabWidth) : 1;
  return col;
}
