/**
 * The diff colours that can be changed on the Settings dialog's Diff page. Each theme keeps its
 * own: `settings.diff.addBgDark` and `settings.diff.addBgLight` and so on, each a `#rrggbbaa`
 * colour or unset for the theme's own colour in `assets/index.css`.
 *
 * A saved colour is set on `<body>` (see `main.ts`), over the theme's colour on `<html>`, so the
 * theme's own colour can still be read from `<html>` to show as the default.
 */
import type { DiffSettings } from "@/lib/bindings";

/** The colours, in the order the Diff page lists them. */
export const DIFF_COLORS = [
  { key: "addBg", cssVar: "--diff-add-bg", label: "Added lines" },
  { key: "delBg", cssVar: "--diff-del-bg", label: "Removed lines" },
  { key: "modBg", cssVar: "--diff-mod-bg", label: "Modified lines (side by side)" },
  { key: "addMark", cssVar: "--diff-add-mark", label: "Added mark" },
  { key: "delMark", cssVar: "--diff-del-mark", label: "Removed mark" },
] as const;

export type DiffColorKey = (typeof DIFF_COLORS)[number]["key"];

/** The setting holding one colour for one theme: `addBgDark`. */
export function diffColorSetting(key: DiffColorKey, dark: boolean): keyof DiffSettings {
  return `${key}${dark ? "Dark" : "Light"}` as keyof DiffSettings;
}

export interface Rgba {
  r: number;
  g: number;
  b: number;
  /** 0 to 1. */
  a: number;
}

/** Reads `#rgb`, `#rrggbb`, `#rrggbbaa`, `rgb()` or `rgba()`; null for anything else. */
export function parseColor(text: string): Rgba | null {
  const t = text.trim().toLowerCase();
  let m = /^#([0-9a-f]{3})$/.exec(t);
  if (m) {
    const [r, g, b] = [...(m[1] as string)].map((c) => parseInt(c + c, 16)) as [number, number, number];
    return { r, g, b, a: 1 };
  }
  m = /^#([0-9a-f]{6})([0-9a-f]{2})?$/.exec(t);
  if (m) {
    const hex = m[1] as string;
    return {
      r: parseInt(hex.slice(0, 2), 16),
      g: parseInt(hex.slice(2, 4), 16),
      b: parseInt(hex.slice(4, 6), 16),
      a: m[2] ? parseInt(m[2], 16) / 255 : 1,
    };
  }
  m = /^rgba?\(\s*(\d+)\s*,\s*(\d+)\s*,\s*(\d+)\s*(?:,\s*([\d.]+)\s*)?\)$/.exec(t);
  if (m) {
    const [r, g, b] = [m[1], m[2], m[3]].map(Number) as [number, number, number];
    const a = m[4] === undefined ? 1 : Number(m[4]);
    if ([r, g, b].some((c) => c > 255) || !(a >= 0 && a <= 1)) return null;
    return { r, g, b, a };
  }
  return null;
}

const hex2 = (n: number): string => Math.round(n).toString(16).padStart(2, "0");

/** `#rrggbb`, without the opacity, for `<input type="color">`. */
export function toHex6(c: Rgba): string {
  return `#${hex2(c.r)}${hex2(c.g)}${hex2(c.b)}`;
}

/** `#rrggbbaa`, the form a colour is saved in. */
export function toHex8(c: Rgba): string {
  return `${toHex6(c)}${hex2(c.a * 255)}`;
}

/** The current theme's own colour for `cssVar`, from `<html>`, under any saved colour on `<body>`. */
export function themeColor(cssVar: string): Rgba | null {
  return parseColor(getComputedStyle(document.documentElement).getPropertyValue(cssVar));
}

/**
 * The saved colours for one theme, as CSS custom properties: only those set, so the rest keep
 * the theme's own colour.
 */
export function diffColorVars(get: (setting: keyof DiffSettings) => unknown, dark: boolean): Record<string, string> {
  const out: Record<string, string> = {};
  for (const { key, cssVar } of DIFF_COLORS) {
    const value = get(diffColorSetting(key, dark));
    const color = typeof value === "string" ? parseColor(value) : null;
    if (color) out[cssVar] = toHex8(color);
  }
  return out;
}
