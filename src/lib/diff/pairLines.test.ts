import { describe, expect, it } from "vitest";
import { pairByPosition, pairBySimilarity } from "./pairLines";

describe("pairBySimilarity", () => {
  it("pairs re-indented lines past an added line, where position would pair them wrongly", () => {
    const removed = ["  return 'transform';", "})();"];
    const added = ["  {", "    }", "    return 'transform';", "  })();"];
    expect(pairByPosition(removed.length, added.length)).toEqual([
      [0, 0],
      [1, 1],
    ]);
    expect(pairBySimilarity(removed, added)).toEqual([
      [0, 2],
      [1, 3],
    ]);
  });

  it("leaves lines unpaired when nothing on the other side is alike", () => {
    expect(pairBySimilarity(["completely different"], ["xyz"])).toEqual([]);
  });

  it("keeps both sides in order", () => {
    const pairs = pairBySimilarity(["alpha one", "beta two", "gamma three"], ["gamma three!", "alpha one!"]);
    for (let i = 1; i < pairs.length; i += 1) {
      expect(pairs[i]![0]).toBeGreaterThan(pairs[i - 1]![0]);
      expect(pairs[i]![1]).toBeGreaterThan(pairs[i - 1]![1]);
    }
  });

  it("falls back to pairing by position for a very large block", () => {
    const many = Array.from({ length: 300 }, (_, i) => `line ${i}`);
    expect(pairBySimilarity(many, many)).toEqual(pairByPosition(300, 300));
  });

  it("falls back to pairing by position for a few very long lines", () => {
    // 100 × 100 lines is well under the cell limit, but each random line has thousands of distinct
    // pieces. Reversed, pairing by similarity could keep only one pair in order; by position, all 100.
    let state = 1;
    const random = () => (state = (state * 48271) % 2147483647);
    const long = () => Array.from({ length: 3000 }, () => String.fromCharCode(0x4e00 + (random() % 2000))).join("");
    const removed = Array.from({ length: 100 }, long);
    const added = [...removed].reverse();
    expect(pairBySimilarity(removed, added)).toEqual(pairByPosition(100, 100));
  });
});
