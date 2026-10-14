import { Text } from "@codemirror/state";
import { describe, expect, it } from "vitest";
import { buildChunks, chunkLines, mapLine, type LineChunk } from "./chunks";

const doc = (lines: string[]): Text => Text.of(lines);

describe("chunkLines", () => {
  it("gives a side's and the result's lines for each difference", () => {
    const side = doc(["a", "b", "c", "d"]);
    const result = doc(["a", "B", "c", "d"]);
    const lines = buildChunks(side, result).map((c) => chunkLines(c, side, result));
    expect(lines).toEqual([{ a0: 1, a1: 2, b0: 1, b1: 2 }]);
  });

  it("gives an empty range on the side that has no lines", () => {
    const side = doc(["a", "c"]);
    const result = doc(["a", "b", "c"]);
    const [c] = buildChunks(side, result).map((ch) => chunkLines(ch, side, result));
    expect(c).toEqual({ a0: 1, a1: 1, b0: 1, b1: 2 });
  });
});

describe("mapLine", () => {
  // Side lines 4-8 became result lines 4-17: 4 lines in place of 13.
  const chunks: LineChunk[] = [{ a0: 4, a1: 8, b0: 4, b1: 17 }];

  it("maps lines before a difference one to one", () => {
    expect(mapLine(chunks, 2, true)).toBe(2);
  });

  it("lines up the tops and bottoms of a difference, in proportion between", () => {
    expect(mapLine(chunks, 4, true)).toBe(4);
    expect(mapLine(chunks, 6, true)).toBeCloseTo(4 + (2 * 13) / 4);
    expect(mapLine(chunks, 8, true)).toBe(17);
  });

  it("shifts lines after a difference by its change in length, both ways", () => {
    expect(mapLine(chunks, 10, true)).toBe(19);
    expect(mapLine(chunks, 19, false)).toBe(10);
  });

  it("maps a line at an insertion point to the start of the insertion", () => {
    const insert: LineChunk[] = [{ a0: 3, a1: 3, b0: 3, b1: 9 }];
    expect(mapLine(insert, 3, true)).toBe(3);
    expect(mapLine(insert, 5, true)).toBe(11);
  });
});
