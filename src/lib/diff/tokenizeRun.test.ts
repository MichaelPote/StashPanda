import { beforeAll, describe, expect, it } from "vitest";
import { createHighlighterCore, type HighlighterCore } from "shiki/core";
import { createJavaScriptRegexEngine } from "shiki/engine/javascript";
import { MAX_SYNTAX_LINE_LENGTH, tokenizeRun } from "./tokenizeRun";

const THEME = "github-dark-default";
let h: HighlighterCore;

beforeAll(async () => {
  h = await createHighlighterCore({
    themes: [(await import("@shikijs/themes/github-dark-default")).default],
    langs: [(await import("@shikijs/langs/javascript")).default],
    engine: createJavaScriptRegexEngine(),
  });
});

/** `const s = "aaa…";`, exactly `length` characters long. */
function statement(length: number): string {
  const frame = 'const s = "";'.length;
  return `const s = "${"a".repeat(length - frame)}";`;
}

const coloured = (line: [string, string | null, number][]) => line.some((t) => t[1] !== null);
const text = (line: [string, string | null, number][]) => line.map((t) => t[0]).join("");

describe("tokenizeRun", () => {
  it("colours a line of exactly the longest length", () => {
    const line = statement(MAX_SYNTAX_LINE_LENGTH);
    expect(line.length).toBe(256);
    const [tokens] = tokenizeRun(h, [line], "javascript", THEME);
    expect(tokens!.length).toBeGreaterThan(1);
    expect(coloured(tokens!)).toBe(true);
    expect(text(tokens!)).toBe(line);
  });

  it("gives a line one character longer back as one plain token", () => {
    const line = statement(MAX_SYNTAX_LINE_LENGTH + 1);
    const [tokens] = tokenizeRun(h, [line], "javascript", THEME);
    expect(tokens).toEqual([[line, null, 0]]);
  });

  it("keeps colouring the lines before and after a long line", () => {
    const long = statement(5000);
    const [before, skipped, after] = tokenizeRun(h, ["const a = 1;", long, "const b = 'two';"], "javascript", THEME);
    expect(coloured(before!)).toBe(true);
    expect(skipped).toEqual([[long, null, 0]]);
    expect(coloured(after!)).toBe(true);
    expect(text(after!)).toBe("const b = 'two';");
  });

  it("gives an empty line no tokens", () => {
    expect(tokenizeRun(h, ["", "let x;"], "javascript", THEME)[0]).toEqual([]);
  });
});
