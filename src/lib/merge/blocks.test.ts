import { history, undo } from "@codemirror/commands";
import { EditorState, Text, type Transaction, type TransactionSpec } from "@codemirror/state";
import { describe, expect, it } from "vitest";
import { blocksField, blockWithMarkers, dismissedChunks, initialBlocks, saveText, setChunkDismissed, setDismissed, setTaken, type BlockSource } from "./blocks";

const source = (over: Partial<BlockSource> = {}): BlockSource => ({
  startLine: 1,
  endLine: 2,
  ours: "ours line\n",
  base: "base line\n",
  theirs: "theirs line\n",
  oursLabel: "HEAD",
  theirsLabel: "feature",
  baseLabel: "abc123",
  ...over,
});

/** An editor state over `text` with its blocks tracked, and a way to apply transactions and undo them. */
function editor(text: string, sources: BlockSource[]) {
  const tracked = blocksField(initialBlocks(Text.of(text.split("\n")), sources));
  let state = EditorState.create({ doc: text, extensions: [history(), tracked.extension] });
  const dispatch = (tr: Transaction): void => {
    state = tr.state;
  };
  return {
    get state() {
      return state;
    },
    blocks: () => state.field(tracked.field),
    apply: (spec: TransactionSpec) => dispatch(state.update(spec)),
    undo: () => undo({ state, dispatch }),
  };
}

describe("blockWithMarkers", () => {
  it("writes the diff3 style when the base is known", () => {
    expect(blockWithMarkers(source())).toBe("<<<<<<< HEAD\nours line\n||||||| abc123\nbase line\n=======\ntheirs line\n>>>>>>> feature\n");
  });

  it("writes the merge style without a base, at the file's marker length", () => {
    expect(blockWithMarkers(source({ base: null, baseLabel: null }), 10)).toBe("<<<<<<<<<< HEAD\nours line\n==========\ntheirs line\n>>>>>>>>>> feature\n");
  });
});

describe("blocksField", () => {
  const text = "first\nbase line\nlast\n";

  it("starts unresolved and resolves when the block's text changes", () => {
    const ed = editor(text, [source()]);
    expect(ed.blocks()[0]?.resolved).toBe(false);
    const from = ed.state.doc.line(2).from;
    ed.apply({ changes: { from, to: from + 4, insert: "BASE" } });
    expect(ed.blocks()[0]?.resolved).toBe(true);
  });

  it("is unresolved again once undo puts the text back", () => {
    const ed = editor(text, [source()]);
    const line = ed.state.doc.line(2);
    ed.apply({ changes: { from: line.from, to: line.to, insert: "ours line" } });
    expect(ed.blocks()[0]?.resolved).toBe(true);
    ed.undo();
    expect(ed.state.doc.toString()).toBe(text);
    expect(ed.blocks()[0]?.resolved).toBe(false);
  });

  it("counts a take as resolving even when the text stays the same, and undo takes it back", () => {
    const ed = editor(text, [source()]);
    ed.apply({ effects: setTaken.of({ id: 0, taken: true }) });
    expect(ed.blocks()[0]?.resolved).toBe(true);
    ed.undo();
    expect(ed.blocks()[0]?.resolved).toBe(false);
  });

  it("resolves a block once both its sides are dismissed, and undo brings one back", () => {
    const ed = editor(text, [source()]);
    ed.apply({ effects: [setDismissed.of({ id: 0, side: "left", dismissed: true }), setChunkDismissed.of({ key: "left:1-2", dismissed: true })] });
    expect(ed.blocks()[0]?.resolved).toBe(false);
    expect(ed.state.field(dismissedChunks).has("left:1-2")).toBe(true);
    ed.apply({ effects: setDismissed.of({ id: 0, side: "right", dismissed: true }) });
    expect(ed.blocks()[0]?.resolved).toBe(true);
    ed.undo();
    expect(ed.blocks()[0]?.resolved).toBe(false);
    ed.undo();
    expect(ed.state.field(dismissedChunks).has("left:1-2")).toBe(false);
  });

  it("grows an empty block to hold what is typed at its place", () => {
    const ed = editor("first\nlast\n", [source({ startLine: 1, endLine: 1, base: null })]);
    const at = ed.state.doc.line(2).from;
    ed.apply({ changes: { from: at, insert: "typed\n" } });
    const b = ed.blocks()[0];
    expect(b && ed.state.doc.sliceString(b.from, b.to)).toBe("typed\n");
    expect(b?.resolved).toBe(true);
  });
});

describe("saveText", () => {
  it("puts unresolved blocks back between markers and leaves resolved ones as they are", () => {
    const ed = editor("first\nbase line\nmiddle\nbase two\nlast\n", [source(), source({ startLine: 3, endLine: 4, ours: "o2\n", base: "base two\n", theirs: "t2\n" })]);
    const line = ed.state.doc.line(2);
    ed.apply({ changes: { from: line.from, to: line.to, insert: "chosen" } });
    const out = saveText(ed.state, ed.blocks(), { crlf: false, bom: false, markerSize: 7 });
    expect(out).toBe("first\nchosen\nmiddle\n<<<<<<< HEAD\no2\n||||||| abc123\nbase two\n=======\nt2\n>>>>>>> feature\nlast\n");
  });

  it("writes CRLF line endings and the byte order mark when the file had them", () => {
    const ed = editor("a\nb\n", []);
    expect(saveText(ed.state, ed.blocks(), { crlf: true, bom: true, markerSize: 7 })).toBe("﻿a\r\nb\r\n");
  });
});
