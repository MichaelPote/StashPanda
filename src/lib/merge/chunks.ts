/**
 * The differences between one side of a conflict (the left or right pane of the merge editor)
 * and the result in the middle, as line ranges, and how a line in one pane maps to the matching
 * line in another so that the panes scroll together.
 */
import { Chunk } from "@codemirror/merge";
import type { ChangeDesc, Text } from "@codemirror/state";

/** One difference, in lines counted from 0, end exclusive: `a` is the side's lines, `b` the result's. */
export interface LineChunk {
  a0: number;
  a1: number;
  b0: number;
  b1: number;
}

/** CodeMirror's comparison settings: give up on very large changes rather than freeze the window. */
const DIFF_CONFIG = { scanLimit: 10_000, timeout: 500 };

/** The differences between `side` (document A) and `result` (document B). */
export function buildChunks(side: Text, result: Text): readonly Chunk[] {
  return Chunk.build(side, result, DIFF_CONFIG);
}

/** The same differences after `changes` were made to the result, without comparing again from the start. */
export function updateChunks(chunks: readonly Chunk[], side: Text, result: Text, changes: ChangeDesc): readonly Chunk[] {
  return Chunk.updateB(chunks, side, result, changes, DIFF_CONFIG);
}

/** The lines a range of a document covers: none when the range is empty. */
function lineSpan(doc: Text, from: number, to: number, end: number): [number, number] {
  const first = doc.lineAt(Math.min(from, doc.length)).number - 1;
  if (from === to) return [first, first];
  return [first, doc.lineAt(Math.min(end, doc.length)).number];
}

/** A chunk as line ranges. */
export function chunkLines(chunk: Chunk, side: Text, result: Text): LineChunk {
  const [a0, a1] = lineSpan(side, chunk.fromA, chunk.toA, chunk.endA);
  const [b0, b1] = lineSpan(result, chunk.fromB, chunk.toB, chunk.endB);
  return { a0, a1, b0, b1 };
}

/**
 * The line (a fraction, counted from 0) in document B that matches `line` in document A, or the
 * other way round when `fromA` is false. Between differences lines map one to one; inside a
 * difference they map in proportion, so a 4-line block and a 13-line block line up at their tops
 * and at their bottoms.
 */
export function mapLine(chunks: readonly LineChunk[], line: number, fromA: boolean): number {
  let shift = 0;
  for (const c of chunks) {
    const [s0, s1, t0, t1] = fromA ? [c.a0, c.a1, c.b0, c.b1] : [c.b0, c.b1, c.a0, c.a1];
    if (line < s0) break;
    if (line < s1 || (s0 === s1 && line === s0)) {
      const span = s1 - s0;
      return t0 + (span > 0 ? ((line - s0) * (t1 - t0)) / span : 0);
    }
    shift = t1 - s1;
  }
  return line + shift;
}
