import { describe, expect, it } from "vitest";
import { merge3, resolveSimpleBlocks, splitLines } from "./autoResolve";

const lines = (...l: string[]): string => l.map((x) => `${x}\n`).join("");

describe("splitLines", () => {
  it("keeps each line's newline, and a last line without one", () => {
    expect(splitLines("a\nb\n")).toEqual(["a\n", "b\n"]);
    expect(splitLines("a\nb")).toEqual(["a\n", "b"]);
    expect(splitLines("")).toEqual([]);
  });
});

describe("merge3", () => {
  const base = splitLines(lines("1", "2", "3", "4", "5"));

  it("conflicts on touching changes the way Git does, and merges them without that rule", () => {
    const ours = splitLines(lines("1", "2", "3", "FOUR", "5"));
    const theirs = splitLines(lines("1", "2", "THREE", "4", "5"));
    expect(merge3(base, ours, theirs, true)?.filter((s) => s.kind === "conflict")).toHaveLength(1);
    expect(merge3(base, ours, theirs, false)).toEqual([{ kind: "same", lines: splitLines(lines("1", "2", "THREE", "FOUR", "5")) }]);
  });

  it("still conflicts where both sides changed the same line differently", () => {
    const ours = splitLines(lines("1", "2", "x", "4", "5"));
    const theirs = splitLines(lines("1", "2", "y", "4", "5"));
    expect(merge3(base, ours, theirs, false)?.filter((s) => s.kind === "conflict")).toHaveLength(1);
  });

  it("takes a change both sides made once", () => {
    const both = splitLines(lines("1", "2", "x", "4", "5"));
    expect(merge3(base, both, both, true)).toEqual([{ kind: "same", lines: both }]);
  });

  it("conflicts on two different insertions at the same place", () => {
    const ours = splitLines(lines("1", "a", "2", "3", "4", "5"));
    const theirs = splitLines(lines("1", "b", "2", "3", "4", "5"));
    expect(merge3(base, ours, theirs, false)?.filter((s) => s.kind === "conflict")).toHaveLength(1);
  });
});

describe("resolveSimpleBlocks", () => {
  // Captured from `git merge` (Git 2.43): main changed line 4 and feature line 3.
  const file = {
    base: lines("one", "two", "three", "four", "five", "six"),
    ours: lines("one", "two", "three", "FOUR", "five", "six"),
    theirs: lines("one", "two", "THREE", "four", "five", "six"),
  };

  it("resolves a block from Git's default style, which carries no base", () => {
    const blocks = [{ ours: lines("three", "FOUR"), base: null, theirs: lines("THREE", "four") }];
    expect(resolveSimpleBlocks({ ...file, blocks })).toEqual([lines("THREE", "FOUR")]);
  });

  it("resolves a block from the diff3 style, whose base must match", () => {
    const blocks = [{ ours: lines("three", "FOUR"), base: lines("three", "four"), theirs: lines("THREE", "four") }];
    expect(resolveSimpleBlocks({ ...file, blocks })).toEqual([lines("THREE", "FOUR")]);
    expect(resolveSimpleBlocks({ ...file, blocks: [{ ...blocks[0]!, base: lines("other") }] })).toEqual([null]);
  });

  it("leaves a real conflict, and a block it cannot find in the merge, alone", () => {
    const real = {
      base: lines("a", "b", "c"),
      ours: lines("a", "X", "c"),
      theirs: lines("a", "Y", "c"),
      blocks: [{ ours: lines("X"), base: null, theirs: lines("Y") }],
    };
    expect(resolveSimpleBlocks(real)).toEqual([null]);
    expect(resolveSimpleBlocks({ ...file, blocks: [{ ours: lines("nothing"), base: null, theirs: lines("like it") }] })).toEqual([null]);
  });

  it("resolves a block Git made of two touching conflicts with short lines between", () => {
    // Captured from `git merge` (Git 2.43): main changed lines 2 and 6, feature lines 3 and 7,
    // and Git wrote one block from line 2 to line 7.
    const joined = {
      base: lines("1", "2", "3", "4", "5", "6", "7", "8"),
      ours: lines("1", "B", "3", "4", "5", "O6", "7", "8"),
      theirs: lines("1", "2", "C", "4", "5", "6", "T7", "8"),
      blocks: [{ ours: lines("B", "3", "4", "5", "O6", "7"), base: null, theirs: lines("2", "C", "4", "5", "6", "T7") }],
    };
    expect(resolveSimpleBlocks(joined)).toEqual([lines("B", "C", "4", "5", "O6", "T7")]);
    // The same, but both sides changed line 6: one half is a real conflict, so the block stays.
    const real = { ...joined, theirs: lines("1", "2", "C", "4", "5", "T6", "7", "8") };
    real.blocks = [{ ours: lines("B", "3", "4", "5", "O6"), base: null, theirs: lines("2", "C", "4", "5", "T6") }];
    expect(resolveSimpleBlocks(real)).toEqual([null]);
  });

  it("matches blocks in order, skipping a conflict that is not one of them", () => {
    const two = {
      base: lines("a", "b", "c", "d", "e", "f", "g", "h", "i", "j", "k"),
      ours: lines("a", "B", "c", "d", "eeeeeeee", "f", "g", "h", "i", "J", "k"),
      theirs: lines("a", "b", "C", "d", "EEEEEEEE", "f", "g", "h", "i", "j", "K"),
    };
    const blocks = [
      { ours: lines("B", "c"), base: null, theirs: lines("b", "C") },
      { ours: lines("J", "k"), base: null, theirs: lines("j", "K") },
    ];
    expect(resolveSimpleBlocks({ ...two, blocks })).toEqual([lines("B", "C"), lines("J", "K")]);
  });

  it("returns null for every block when a side is missing", () => {
    expect(resolveSimpleBlocks({ ...file, theirs: null, blocks: [{ ours: "", base: null, theirs: "" }] })).toEqual([null]);
  });
});
