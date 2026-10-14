import { describe, expect, it } from "vitest";
import { findInText, markFound } from "./find";
import type { Span } from "./syntax";

const span = (text: string, color: string | null = null, changed = false): Span => ({ text, changed, color, italic: false, bold: false });

describe("findInText", () => {
  it("finds every match, ignoring case, without overlaps", () => {
    expect(findInText("Foo foo FOO", "foo")).toEqual([
      [0, 3],
      [4, 7],
      [8, 11],
    ]);
    expect(findInText("aaaa", "aa")).toEqual([
      [0, 2],
      [2, 4],
    ]);
  });

  it("finds nothing for an empty search", () => {
    expect(findInText("text", "")).toEqual([]);
  });

  it("keeps offsets right when lower-casing would change the text's length", () => {
    // "İ" lower-cases to two characters; that line is searched with case taken into account.
    const text = "İstanbul and more";
    expect(findInText(text, "more")).toEqual([[13, 17]]);
  });
});

describe("markFound", () => {
  it("cuts coloured pieces at the edges of a match and marks the current one", () => {
    const spans = [span("const ", "red"), span("value", "blue"), span(" = 1;")];
    const out = markFound(spans, [{ start: 4, end: 8, ordinal: 0 }], 0);
    expect(out.map((s) => [s.text, s.color, s.found ?? 0])).toEqual([
      ["cons", "red", 0],
      ["t ", "red", 2],
      ["va", "blue", 2],
      ["lue", "blue", 0],
      [" = 1;", null, 0],
    ]);
  });

  it("keeps a changed word's mark on the pieces of a match inside it", () => {
    const out = markFound([span("abcdef", null, true)], [{ start: 2, end: 4, ordinal: 3 }], 0);
    expect(out.map((s) => [s.text, s.changed, s.found ?? 0])).toEqual([
      ["ab", true, 0],
      ["cd", true, 1],
      ["ef", true, 0],
    ]);
  });
});
