/**
 * Colours one run of lines with Shiki. Kept apart from `syntax.worker.ts`, which only runs in a
 * worker, so that it can be tested.
 */
import type { HighlighterCore } from "shiki/core";

/** One token: its text, its colour (`#rrggbb`, or null for the theme's default) and font style bits (1 italic, 2 bold). */
export type RawToken = [string, string | null, number];

/**
 * A line longer than this many characters is not coloured: it comes back as one plain token.
 * One very long line (minified code, an inlined image, a lock file's hash list) becomes
 * thousands of coloured pieces, each an element of its own in the diff viewer, the Blame dialog
 * and the merge editor, and laying those out slows the whole window down.
 */
export const MAX_SYNTAX_LINE_LENGTH = 256;

/**
 * The tokens of each of `lines`, coloured as `lang` in `theme`, both already loaded into `h`.
 * The lines are coloured together, since a comment or a string can run over several of them.
 * A line over `MAX_SYNTAX_LINE_LENGTH` is skipped: Shiki carries on after it with the state it
 * had before, so the lines that follow keep their colours.
 */
export function tokenizeRun(h: HighlighterCore, lines: string[], lang: string, theme: string): RawToken[][] {
  // Shiki leaves out lines at or over its limit, hence the one more.
  const tokens = h.codeToTokensBase(lines.join("\n"), { lang, theme, tokenizeMaxLineLength: MAX_SYNTAX_LINE_LENGTH + 1 });
  // A line Shiki left out has the empty string for its colour.
  return tokens.map((line) => line.map((t): RawToken => [t.content, t.color || null, t.fontStyle ?? 0]));
}
