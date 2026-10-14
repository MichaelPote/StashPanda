import { describe, expect, it } from "vitest";
import { BandBuilder, sideLine, sideOffsets } from "./sideLayout";

/** A hunk header, 3 lines both sides have, a change of 4 lines into 13, then 3 more alike. */
function sample() {
  const b = new BandBuilder();
  b.add("hunk", 1, 1);
  b.add("same", 3, 3);
  b.add("change", 4, 13);
  b.add("same", 3, 3);
  return b;
}

describe("BandBuilder", () => {
  it("counts a change as many rows as its longer side, and merges neighbouring same lines", () => {
    const b = new BandBuilder();
    b.add("same", 2, 2);
    b.add("same", 1, 1);
    b.add("change", 4, 13);
    expect(b.bands).toHaveLength(2);
    expect(b.bands[0]).toMatchObject({ y: 0, h: 3, l: 0, lh: 3, r: 0, rh: 3 });
    expect(b.bands[1]).toMatchObject({ y: 3, h: 13, l: 3, lh: 4, r: 3, rh: 13 });
    expect(b.rows).toBe(16);
  });
});

describe("sideLine", () => {
  const b = sample();

  it("moves the shorter side of a change in proportion", () => {
    // The change starts at row 4; halfway through its 13 rows the left side is 2 of its 4 lines in.
    expect(sideLine(b.bands, 4 + 6.5, "left")).toBeCloseTo(4 + 2);
    expect(sideLine(b.bands, 4 + 6.5, "right")).toBeCloseTo(4 + 6.5);
  });

  it("lines the sides up again after the change", () => {
    expect(sideLine(b.bands, 17, "left")).toBe(8);
    expect(sideLine(b.bands, 17, "right")).toBe(17);
  });
});

describe("sideOffsets", () => {
  const b = sample();
  const row = 16;
  const view = 100;

  it("starts both sides at the top", () => {
    expect(sideOffsets(b.bands, b.rows, row, 0, view)).toEqual({ left: 0, right: 0 });
  });

  it("ends both sides at the bottom", () => {
    const max = b.rows * row - view;
    const off = sideOffsets(b.bands, b.rows, row, max, view);
    const leftLines = 1 + 3 + 4 + 3;
    expect(off.left + leftLines * row).toBeCloseTo(b.rows * row);
    expect(off.right + b.rows * row).toBeCloseTo(b.rows * row);
  });
});
