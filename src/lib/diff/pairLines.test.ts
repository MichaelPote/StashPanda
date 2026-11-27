import { describe, expect, it } from "vitest";
import { pairBudget, pairByPosition, pairBySimilarity } from "./pairLines";

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

  it("pairs by position at once when every line is alike the one in its place", () => {
    // A rename over a run of lines: as many added as removed, each differing in one word.
    const removed = Array.from({ length: 150 }, (_, i) => `const value${i} = compute(${i}, "item ${i}"); // line ${i}`);
    const added = removed.map((l) => l.replace("item", "entry number"));
    const budget = pairBudget();
    const before = budget.lookups;
    expect(pairBySimilarity(removed, added, budget)).toEqual(pairByPosition(150, 150));
    expect(budget.lookups).toBe(before);
  });

  it("still matches by similarity when lines of an equal-sized block are not alike in position", () => {
    // Three removed, three added, but the added ones are the removed ones moved down by one.
    const removed = ["alpha = load(first);", "beta = load(second);", "gamma = load(third);"];
    const added = ["zeta: 9000 %% ~~~~", "alpha = load(first, true);", "beta = load(second, true);"];
    expect(pairBySimilarity(removed, added)).toEqual([
      [0, 1],
      [1, 2],
    ]);
  });

  it("stops matching by similarity once the diff's budget is spent, for every later block", () => {
    const removed = ["  return 'transform';", "})();"];
    const added = ["  {", "    }", "    return 'transform';", "  })();"];
    const matched = [
      [0, 2],
      [1, 3],
    ];
    const budget = pairBudget();
    expect(pairBySimilarity(removed, added, budget)).toEqual(matched);
    expect(budget.lookups).toBeLessThan(pairBudget().lookups);

    // A budget too small for the block: by position, and nothing left for the blocks after it.
    const small = { lookups: 3 };
    expect(pairBySimilarity(removed, added, small)).toEqual(pairByPosition(2, 4));
    expect(small.lookups).toBe(0);
    expect(pairBySimilarity(removed, added, small)).toEqual(pairByPosition(2, 4));
  });

  it("bounds the work for a diff of many blocks that each stay under the per-block limits", () => {
    // 120 blocks of 100 removed and 110 added lines that are not alike in position.
    let state = 7;
    const random = () => (state = (state * 48271) % 2147483647);
    const line = () => Array.from({ length: 12 }, () => `w${random() % 500}`).join(" ");
    const budget = pairBudget();
    const started = performance.now();
    let byPosition = 0;
    for (let b = 0; b < 120; b += 1) {
      const removed = Array.from({ length: 100 }, line);
      const added = Array.from({ length: 110 }, line);
      const pairs = pairBySimilarity(removed, added, budget);
      if (JSON.stringify(pairs) === JSON.stringify(pairByPosition(100, 110))) byPosition += 1;
    }
    expect(budget.lookups).toBe(0);
    expect(byPosition).toBeGreaterThan(100);
    // Without the budget this takes several seconds; with it, well under one.
    expect(performance.now() - started).toBeLessThan(1500);
  });
});
