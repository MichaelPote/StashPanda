/**
 * The conflict blocks of the merge editor's result pane: where each one is in the document as the
 * user edits it, and whether it has been resolved. Git's conflict markers are not in the
 * document; each block holds its two sides' text, and a block still unresolved when the file is
 * saved is written back with the markers around them, so a conflict is never dropped silently.
 */
import { StateEffect, StateField, type EditorState, type Extension, type Text } from "@codemirror/state";
import { invertedEffects } from "@codemirror/commands";

/** What the backend says about one block (the fields of `ConflictBlock` this needs). */
export interface BlockSource {
  startLine: number;
  endLine: number;
  ours: string;
  base: string | null;
  theirs: string;
  oursLabel: string;
  theirsLabel: string;
  baseLabel: string | null;
}

/** A block in the result document: `from` and `to` are document positions covering whole lines. */
export interface MergeBlock {
  id: number;
  from: number;
  to: number;
  /** The block's text when the editor opened: its base text, or nothing. */
  original: string;
  /** Filled by an arrow or "take for all", which counts even when the text it put there is the original. */
  taken: boolean;
  /** Each side's lines were dismissed with "×": the user does not want them in the result. */
  dismissed: { left: boolean; right: boolean };
  /** Its text differs from the original, it was taken, or both sides were dismissed. */
  resolved: boolean;
  source: BlockSource;
}

/**
 * Marks a block as taken or not. The arrows and "take for all" send it with their change, and
 * undo sends it back the other way (see `blocksField`), so undoing a take unresolves the block.
 */
export const setTaken = StateEffect.define<{ id: number; taken: boolean }>();

/** Marks one side of a block as dismissed or not, as "×" does; undo sends it back the other way. */
export const setDismissed = StateEffect.define<{ id: number; side: "left" | "right"; dismissed: boolean }>();

/**
 * Dismisses a difference between one side and the result, or brings it back. `key` names it by
 * its side and that side's lines (`left:3-5`); a side pane never changes, so the key holds as
 * the result is edited. Undo sends it back the other way.
 */
export const setChunkDismissed = StateEffect.define<{ key: string; dismissed: boolean }>();

/** The differences dismissed with "×", by key. */
export const dismissedChunks = StateField.define<ReadonlySet<string>>({
  create: () => new Set(),
  update(set, tr) {
    let next = set;
    for (const e of tr.effects) {
      if (!e.is(setChunkDismissed) || next.has(e.value.key) === e.value.dismissed) continue;
      const copy = new Set(next);
      if (e.value.dismissed) copy.add(e.value.key);
      else copy.delete(e.value.key);
      next = copy;
    }
    return next;
  },
});

/** The document range of lines `[start, end)` of `doc`, including the newline after the last. */
function linesRange(doc: Text, start: number, end: number): [number, number] {
  const from = start < doc.lines ? doc.line(start + 1).from : doc.length;
  if (end <= start) return [from, from];
  const to = end < doc.lines ? doc.line(end + 1).from : doc.length;
  return [from, to];
}

/** The blocks of a freshly loaded result document. */
export function initialBlocks(doc: Text, sources: readonly BlockSource[]): MergeBlock[] {
  return sources.map((source, id) => {
    const [from, to] = linesRange(doc, source.startLine, source.endLine);
    return { id, from, to, original: doc.sliceString(from, to), taken: false, dismissed: { left: false, right: false }, resolved: false, source };
  });
}

/**
 * The blocks, mapped through every edit, as a state field and the undo support it needs, with
 * the dismissed differences. Typing at a block's edges grows it, so an empty block (no base
 * text) holds what is typed at its place. A block is resolved while its text differs from the
 * original, it was taken, or both its sides were dismissed (the result keeps what it has); undo
 * restores the text and the marks, so it unresolves the block again.
 */
export function blocksField(initial: MergeBlock[]): { field: StateField<MergeBlock[]>; extension: Extension } {
  const field = StateField.define<MergeBlock[]>({
    create: () => initial,
    update(blocks, tr) {
      if (!tr.docChanged && !tr.effects.some((e) => e.is(setTaken) || e.is(setDismissed))) return blocks;
      return blocks.map((b) => {
        let { from, to, taken, dismissed } = b;
        if (tr.docChanged) {
          from = tr.changes.mapPos(b.from, -1);
          to = Math.max(from, tr.changes.mapPos(b.to, 1));
        }
        for (const e of tr.effects) {
          if (e.is(setTaken) && e.value.id === b.id) taken = e.value.taken;
          if (e.is(setDismissed) && e.value.id === b.id) dismissed = { ...dismissed, [e.value.side]: e.value.dismissed };
        }
        const resolved = taken || (dismissed.left && dismissed.right) || tr.newDoc.sliceString(from, to) !== b.original;
        return { ...b, from, to, taken, dismissed, resolved };
      });
    },
  });
  // Undo and redo send each mark back to what it was before the transaction.
  const undoMarks = invertedEffects.of((tr) => {
    const before = tr.startState.field(field);
    const out: StateEffect<unknown>[] = [];
    for (const e of tr.effects) {
      if (e.is(setTaken)) {
        out.push(setTaken.of({ id: e.value.id, taken: before.find((b) => b.id === e.value.id)?.taken ?? false }));
      } else if (e.is(setDismissed)) {
        const was = before.find((b) => b.id === e.value.id)?.dismissed[e.value.side] ?? false;
        out.push(setDismissed.of({ ...e.value, dismissed: was }));
      } else if (e.is(setChunkDismissed)) {
        out.push(setChunkDismissed.of({ key: e.value.key, dismissed: tr.startState.field(dismissedChunks).has(e.value.key) }));
      }
    }
    return out;
  });
  return { field, extension: [field, dismissedChunks, undoMarks] };
}

/** The markers Git writes, at its default length of seven characters. */
const MARK = { ours: "<<<<<<<", base: "|||||||", split: "=======", theirs: ">>>>>>>" };

/** A block as Git wrote it: both sides, and the base when it was known, between conflict markers. */
export function blockWithMarkers(source: BlockSource): string {
  const label = (text: string | null): string => (text ? ` ${text}` : "");
  let out = `${MARK.ours}${label(source.oursLabel)}\n${source.ours}`;
  if (source.base !== null) out += `${MARK.base}${label(source.baseLabel)}\n${source.base}`;
  out += `${MARK.split}\n${source.theirs}${MARK.theirs}${label(source.theirsLabel)}\n`;
  return out;
}

/** How to write the file back: its line endings, byte order mark, and whether the final newline is kept as the document has it. */
export interface SaveFormat {
  crlf: boolean;
  bom: boolean;
}

/**
 * The text to save: the document, with each unresolved block put back between Git's markers,
 * then the file's own line endings and byte order mark.
 */
export function saveText(state: EditorState, blocks: readonly MergeBlock[], format: SaveFormat): string {
  const doc = state.doc.toString();
  let out = "";
  let at = 0;
  for (const b of [...blocks].sort((x, y) => x.from - y.from)) {
    if (b.resolved) continue;
    out += doc.slice(at, b.from) + blockWithMarkers(b.source);
    at = b.to;
  }
  out += doc.slice(at);
  if (format.crlf) out = out.replace(/\n/g, "\r\n");
  return format.bom ? `﻿${out}` : out;
}
