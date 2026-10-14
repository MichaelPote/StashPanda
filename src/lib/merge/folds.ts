/**
 * Folding unchanged lines in the merge editor: a long run of lines that is the same in all three
 * panes is replaced by one row saying how many lines it hides, with a button to show them. The
 * dialog works out the runs and puts the same folds in every pane, each one row high, so the
 * panes stay lined up.
 */
import { StateEffect, StateField, type Extension, type Range, type Text } from "@codemirror/state";
import { BlockType, Decoration, EditorView, WidgetType, type BlockInfo, type DecorationSet } from "@codemirror/view";

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

/** A line's text part: a line block also holds any block widget drawn above the line. */
function textPart(block: BlockInfo): BlockInfo {
  const parts = block.type;
  return Array.isArray(parts) ? ((parts as readonly BlockInfo[]).find((b) => b.type === BlockType.Text) ?? block) : block;
}

/**
 * The top of line `line` (a fraction, counted from 0), from the top of the document, with folds
 * taken into account: a line inside a fold sits at its share of the fold's one row. It is the
 * top of the line's text, below any block widget drawn above it (see `lineAbove`).
 */
export function lineTop(view: EditorView, line: number): number {
  const doc = view.state.doc;
  if (line >= doc.lines) return view.lineBlockAt(doc.length).bottom;
  const whole = Math.max(0, Math.floor(line));
  const block = view.lineBlockAt(doc.line(whole + 1).from);
  const first = doc.lineAt(block.from).number - 1;
  const span = Math.max(1, doc.lineAt(block.to).number - first);
  const text = textPart(block);
  return text.top + ((Math.max(0, line) - first) / span) * text.height;
}

/** The top of line `line` (counted from 0) including any block widget drawn above it, such as an empty conflict's placeholder. */
export function lineAbove(view: EditorView, line: number): number {
  const doc = view.state.doc;
  if (line >= doc.lines) return lineTop(view, line);
  return view.lineBlockAt(doc.line(Math.max(0, Math.floor(line)) + 1).from).top;
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
