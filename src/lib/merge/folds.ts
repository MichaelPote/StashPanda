/**
 * Folding unchanged lines in the merge editor: a long run of lines that is the same in all three
 * panes is replaced by one row saying how many lines it hides, with a button to show them. The
 * dialog works out the runs and puts the same folds in every pane, each one row high, so the
 * panes stay lined up.
 */
import { StateEffect, StateField, type Extension, type Range, type Text } from "@codemirror/state";
import { Decoration, EditorView, WidgetType, type DecorationSet } from "@codemirror/view";

/** A run of lines `[from, to)`, counted from 0, to hide; `key` names it in every pane. */
export interface FoldRange {
  key: string;
  from: number;
  to: number;
}

class FoldWidget extends WidgetType {
  constructor(
    readonly count: number,
    readonly key: string,
    readonly rowPx: number,
    readonly onShow: (key: string) => void,
  ) {
    super();
  }

  eq(other: FoldWidget): boolean {
    return other.count === this.count && other.key === this.key && other.rowPx === this.rowPx;
  }

  toDOM(): HTMLElement {
    const row = document.createElement("div");
    row.className = "merge-fold";
    row.dataset.mergeFold = this.key;
    const text = document.createElement("span");
    text.textContent = `${this.count.toLocaleString()} unchanged ${this.count === 1 ? "line" : "lines"} hidden`;
    const show = document.createElement("button");
    show.type = "button";
    show.className = "merge-fold-show";
    show.textContent = "Show";
    show.addEventListener("click", () => this.onShow(this.key));
    row.append(text, show);
    return row;
  }

  /** One row, as tall as a line, so that CodeMirror places the lines after it right before measuring. */
  get estimatedHeight(): number {
    return this.rowPx;
  }

  /** The Show button handles its own clicks; the editor leaves them alone. */
  ignoreEvent(): boolean {
    return true;
  }
}

const setFolds = StateEffect.define<DecorationSet>();

const foldField = StateField.define<DecorationSet>({
  create: () => Decoration.none,
  update(set, tr) {
    let next = set.map(tr.changes);
    for (const e of tr.effects) if (e.is(setFolds)) next = e.value;
    return next;
  },
  provide: (f) => EditorView.decorations.from(f),
});

/** The extension a pane needs for `foldsEffect` to hide its lines. */
export function folds(): Extension {
  return foldField;
}

/** The effect that replaces a pane's folds with `ranges`; `onShow` runs when one's Show is clicked. */
export function foldsEffect(doc: Text, ranges: readonly FoldRange[], rowPx: number, onShow: (key: string) => void) {
  const decorations: Range<Decoration>[] = [];
  for (const f of ranges) {
    if (f.to <= f.from || f.to > doc.lines) continue;
    const widget = new FoldWidget(f.to - f.from, f.key, rowPx, onShow);
    decorations.push(Decoration.replace({ widget, block: true }).range(doc.line(f.from + 1).from, doc.line(f.to).to));
  }
  return setFolds.of(Decoration.set(decorations, true));
}

/**
 * The top of line `line` (a fraction, counted from 0), from the top of the document, with folds
 * taken into account: a line inside a fold sits at its share of the fold's one row.
 */
export function lineTop(view: EditorView, line: number): number {
  const doc = view.state.doc;
  if (line <= 0) return 0;
  if (line >= doc.lines) return view.lineBlockAt(doc.length).bottom;
  const whole = Math.floor(line);
  const block = view.lineBlockAt(doc.line(whole + 1).from);
  const first = doc.lineAt(block.from).number - 1;
  const span = Math.max(1, doc.lineAt(block.to).number - first);
  return block.top + ((line - first) / span) * block.height;
}

/** The line (a fraction, counted from 0) at height `y` from the top of the document: the inverse of `lineTop`. */
export function lineAtHeight(view: EditorView, y: number): number {
  const doc = view.state.doc;
  const block = view.lineBlockAtHeight(Math.max(0, y));
  const first = doc.lineAt(block.from).number - 1;
  const span = doc.lineAt(block.to).number - first;
  const into = block.height > 0 ? Math.min(1, Math.max(0, (y - block.top) / block.height)) : 0;
  return first + into * span;
}
