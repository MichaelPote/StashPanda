import { describe, expect, it } from "vitest";
import { matchSegments, oidSegments } from "./find";

describe("marking the commit search's matches in a row", () => {
  it("cuts the text around every match, ignoring case and keeping the text's own case", () => {
    expect(matchSegments("Fix the parser; PARSER tests", "parser")).toEqual([
      { text: "Fix the ", match: false },
      { text: "parser", match: true },
      { text: "; ", match: false },
      { text: "PARSER", match: true },
      { text: " tests", match: false },
    ]);
  });

  it("marks a match at either end, and the whole text", () => {
    expect(matchSegments("dev", "DEV")).toEqual([{ text: "dev", match: true }]);
    expect(matchSegments("ab-ab", "ab")).toEqual([
      { text: "ab", match: true },
      { text: "-", match: false },
      { text: "ab", match: true },
    ]);
  });

  it("leaves the text unmarked for no match, an empty search or a search of spaces", () => {
    expect(matchSegments("Fix the parser", "lexer")).toEqual([{ text: "Fix the parser", match: false }]);
    expect(matchSegments("Fix the parser", "")).toEqual([{ text: "Fix the parser", match: false }]);
    expect(matchSegments("Fix the parser", "   ")).toEqual([{ text: "Fix the parser", match: false }]);
  });

  it("finds matches beyond ASCII", () => {
    expect(matchSegments("Zoë Example", "ZOË")).toEqual([
      { text: "Zoë", match: true },
      { text: " Example", match: false },
    ]);
  });

  it("marks the start of the shown commit ID when the search is the start of the ID", () => {
    const oid = "327ebf9a0c1d2e3f405162738495a6b7c8d9e0f1";
    expect(oidSegments("327ebf9", oid, "327E")).toEqual([
      { text: "327e", match: true },
      { text: "bf9", match: false },
    ]);
    expect(oidSegments("327ebf9", oid, "327ebf9a0c")).toEqual([{ text: "327ebf9", match: true }]);
    expect(oidSegments("327ebf9", oid, "ebf9")).toEqual([{ text: "327ebf9", match: false }]);
    expect(oidSegments("327ebf9", oid, "parser")).toEqual([{ text: "327ebf9", match: false }]);
  });
});
