/**
 * Syntax colours in a merge editor pane, from the same Shiki worker as the diff viewer, drawn as
 * CodeMirror mark decorations. The colours are asked for once for a read-only pane and again
 * after the user stops typing for the result; meanwhile the old ones move with the edits.
 */
import { StateEffect, StateField, type Extension, type Range, type Text } from "@codemirror/state";
import { Decoration, EditorView, ViewPlugin, type DecorationSet, type ViewUpdate } from "@codemirror/view";
import { highlightRuns } from "@/lib/diff/syntax";
import type { RawToken, SyntaxTheme } from "@/lib/diff/syntax.worker";

/** Past this many lines a pane stays plain, as the diff viewer does. */
const MAX_SYNTAX_LINES = 20_000;
/** How long the result pane waits after the last edit before asking for colours again. */
const RECOLOUR_MS = 300;

const setColours = StateEffect.define<DecorationSet>();

const colourField = StateField.define<DecorationSet>({
  create: () => Decoration.none,
  update(set, tr) {
    let next = set.map(tr.changes);
    for (const e of tr.effects) if (e.is(setColours)) next = e.value;
    return next;
  },
  provide: (f) => EditorView.decorations.from(f),
});

/** One mark per distinct style, shared by every token drawn with it. */
const marks = new Map<string, Decoration>();
function markFor(color: string | null, style: number): Decoration | null {
  if (!color && style === 0) return null;
  const css = [color ? `color:${color}` : "", style & 1 ? "font-style:italic" : "", style & 2 ? "font-weight:600" : ""].filter(Boolean).join(";");
  let mark = marks.get(css);
  if (!mark) marks.set(css, (mark = Decoration.mark({ attributes: { style: css } })));
  return mark;
}

function decorationsFor(doc: Text, lines: RawToken[][]): DecorationSet {
  const ranges: Range<Decoration>[] = [];
  lines.forEach((tokens, i) => {
    if (i >= doc.lines) return;
    let pos = doc.line(i + 1).from;
    const end = doc.line(i + 1).to;
    for (const [text, color, style] of tokens) {
      const to = Math.min(end, pos + text.length);
      const mark = markFor(color, style);
      if (mark && to > pos) ranges.push(mark.range(pos, to));
      pos = to;
    }
  });
  return Decoration.set(ranges, true);
}

/**
 * Colours the pane's text by the language of `path` in `theme`. The document as it was when the
 * colours were asked for must still be the document when they arrive; otherwise they are
 * dropped and asked for again.
 */
export function syntaxColours(path: string, theme: SyntaxTheme): Extension {
  const plugin = ViewPlugin.fromClass(
    class {
      private timer: ReturnType<typeof setTimeout> | null = null;
      private ticket = 0;
      constructor(private view: EditorView) {
        this.request();
      }
      update(update: ViewUpdate): void {
        if (!update.docChanged) return;
        if (this.timer !== null) clearTimeout(this.timer);
        this.timer = setTimeout(() => this.request(), RECOLOUR_MS);
      }
      destroy(): void {
        if (this.timer !== null) clearTimeout(this.timer);
        this.ticket += 1;
      }
      private request(): void {
        this.timer = null;
        const doc = this.view.state.doc;
        if (doc.lines > MAX_SYNTAX_LINES) return;
        const ticket = (this.ticket += 1);
        const lines = doc.toString().split("\n");
        highlightRuns(path, theme, [lines])
          .then((tokens) => {
            if (ticket !== this.ticket || this.view.state.doc !== doc || !tokens) return;
            this.view.dispatch({ effects: setColours.of(decorationsFor(doc, tokens[0] ?? [])) });
          })
          .catch(() => {
            // A language that fails to load leaves the text plain.
          });
      }
    },
  );
  return [colourField, plugin];
}
