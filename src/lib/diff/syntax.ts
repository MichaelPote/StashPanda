/**
 * Syntax colours for the diff viewer: the one worker that colours text (`syntax.worker.ts`, which
 * also works out the file's language), and how its tokens combine with the changed-word segments.
 */
import type { Segment } from "@/lib/diff/parseUnified";
import type { RawToken, SyntaxRequest, SyntaxResponse, SyntaxTheme } from "@/lib/diff/syntax.worker";

export function themeFor(dark: boolean): SyntaxTheme {
  return dark ? "github-dark-default" : "github-light-default";
}

let worker: Worker | null = null;
let nextId = 1;
const pending = new Map<number, { resolve: (t: RawToken[][][] | null) => void; reject: (e: Error) => void }>();

function getWorker(): Worker {
  if (worker) return worker;
  worker = new Worker(new URL("./syntax.worker.ts", import.meta.url), { type: "module" });
  worker.onmessage = (event: MessageEvent<SyntaxResponse>) => {
    const msg = event.data;
    const p = pending.get(msg.id);
    if (!p) return;
    pending.delete(msg.id);
    if ("error" in msg) p.reject(new Error(msg.error));
    else p.resolve(msg.tokens);
  };
  return worker;
}

/**
 * The tokens of every line of every run, coloured with `theme` in the language of `path` (from
 * its file name or extension). Null when the language is not known: the text stays plain.
 */
export function highlightRuns(path: string, theme: SyntaxTheme, runs: string[][]): Promise<RawToken[][][] | null> {
  const id = nextId++;
  return new Promise((resolve, reject) => {
    pending.set(id, { resolve, reject });
    getWorker().postMessage({ id, path, theme, runs } satisfies SyntaxRequest);
  });
}

/** A piece of a line as the viewer draws it: coloured by its token, and marked when it is a changed word. */
export interface Span {
  text: string;
  changed: boolean;
  color: string | null;
  italic: boolean;
  bold: boolean;
}

/** The inline style that draws a span's colour and font style, or nothing for plain text. */
export function spanStyle(span: Span): Record<string, string> | undefined {
  if (!span.color && !span.italic && !span.bold) return undefined;
  return {
    ...(span.color ? { color: span.color } : {}),
    ...(span.italic ? { fontStyle: "italic" } : {}),
    ...(span.bold ? { fontWeight: "600" } : {}),
  };
}

/**
 * Cuts a line's syntax tokens and its changed-word segments into spans that each have one colour
 * and one changed state. Both describe the same text; when they do not (the file changed between
 * the two), the segments alone are used.
 */
export function mergeSpans(tokens: RawToken[] | undefined, segments: Segment[]): Span[] {
  const plain = (): Span[] => segments.map((s) => ({ text: s.text, changed: s.changed, color: null, italic: false, bold: false }));
  if (!tokens || tokens.length === 0) return plain();
  if (tokens.map((t) => t[0]).join("") !== segments.map((s) => s.text).join("")) return plain();
  const out: Span[] = [];
  let ti = 0;
  let si = 0;
  let tOff = 0;
  let sOff = 0;
  while (ti < tokens.length && si < segments.length) {
    const [text, color, style] = tokens[ti] as RawToken;
    const seg = segments[si] as Segment;
    const take = Math.min(text.length - tOff, seg.text.length - sOff);
    if (take > 0) {
      out.push({ text: text.slice(tOff, tOff + take), changed: seg.changed, color, italic: (style & 1) !== 0, bold: (style & 2) !== 0 });
    }
    tOff += take;
    sOff += take;
    if (tOff >= text.length) {
      ti += 1;
      tOff = 0;
    }
    if (sOff >= seg.text.length) {
      si += 1;
      sOff = 0;
    }
  }
  return out;
}
