/**
 * Colours one run of lines with Shiki. Kept apart from `syntax.worker.ts`, which only runs in a
 * worker, so that it can be tested.
 */
import type { HighlighterCore } from "shiki/core";

/** One token: its text, its colour (`#rrggbb`, or null for the theme's default) and font style bits (1 italic, 2 bold). */
export type RawToken = [string, string | null, number];

/**
 * The longest line that is coloured unless the `diff.syntaxMaxLineLength` setting says otherwise,
 * in characters. A longer line comes back as one plain token. One very long line (minified code,
 * an inlined image, a lock file's hash list) becomes thousands of coloured pieces, each an
 * element of its own in the diff viewer, the Blame dialog and the merge editor, and laying those
 * out slows the whole window down. 500 is also where the marking of changed words gives up on a
 * line (`TOKEN_CAP` in `parseUnified.ts`, counted in words there).
 */
export const DEFAULT_SYNTAX_MAX_LINE_LENGTH = 500;

/**
 * The tokens of each of `lines`, coloured as `lang` in `theme`, both already loaded into `h`.
 * The lines are coloured together, since a comment or a string can run over several of them.
 * A line over `maxLineLength` characters is skipped: Shiki carries on after it with the state
 * it had before, so the lines that follow keep their colours.
 */
export function tokenizeRun(h: HighlighterCore, lines: string[], lang: string, theme: string, maxLineLength: number): RawToken[][] {
  // Shiki leaves out lines at or over its limit, hence the one more.
  const tokens = h.codeToTokensBase(lines.join("\n"), { lang, theme, tokenizeMaxLineLength: maxLineLength + 1 });
  // A line Shiki left out has the empty string for its colour.
  return tokens.map((line) => line.map((t): RawToken => [t.content, t.color || null, t.fontStyle ?? 0]));
}
