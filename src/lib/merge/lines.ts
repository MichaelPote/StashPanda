/**
 * Line backgrounds in a merge editor pane: the lines of each difference from the result, and of
 * each conflict block, in the colours the curves between the panes use. The dialog works out the
 * ranges and sets them whenever the differences change.
 */
import { StateEffect, StateField, type Extension, type Range, type Text } from "@codemirror/state";
import { Decoration, EditorView, type DecorationSet } from "@codemirror/view";

/** Lines `[from, to)`, counted from 0, drawn with `kind`'s background. */
export interface LineMark {
  from: number;
  to: number;
  kind: "change" | "conflict";
}

const setLineMarks = StateEffect.define<DecorationSet>();

const lineMarkField = StateField.define<DecorationSet>({
  create: () => Decoration.none,
  update(set, tr) {
    let next = set.map(tr.changes);
    for (const e of tr.effects) if (e.is(setLineMarks)) next = e.value;
    return next;
  },
  provide: (f) => EditorView.decorations.from(f),
});

const LINE_CLASS: Record<LineMark["kind"], Decoration> = {
  change: Decoration.line({ class: "merge-line-change" }),
  conflict: Decoration.line({ class: "merge-line-conflict" }),
};

/** The extension a pane needs for `lineMarksEffect` to draw. */
export function lineMarks(): Extension {
  return lineMarkField;
}

/** The effect that replaces a pane's line backgrounds with `marks`. */
export function lineMarksEffect(doc: Text, marks: readonly LineMark[]) {
  const ranges: Range<Decoration>[] = [];
  for (const m of marks) {
    for (let line = m.from; line < m.to && line < doc.lines; line += 1) ranges.push(LINE_CLASS[m.kind].range(doc.line(line + 1).from));
  }
  return setLineMarks.of(Decoration.set(ranges, true));
}
