import { describe, expect, it } from "vitest";
import type { Edge, GraphRow } from "@/lib/bindings";
import { laneExtent, messageIndent, LANE_WIDTH } from "./paths";

function row(lane: number, edges: [number, number][], dangling = false): GraphRow {
  return {
    lane,
    edges: edges.map(([fromLane, toLane]): Edge => ({ fromLane, toLane, colour: 0, kind: dangling ? "dangling" : "pass" }) as Edge),
  } as unknown as GraphRow;
}

describe("how far the lanes reach on a row, for the graph inside the message column", () => {
  it("counts the row's dot and the lanes passing through it", () => {
    expect(laneExtent(row(0, [[0, 0]]), undefined)).toBe(0);
    expect(laneExtent(row(0, [[0, 0], [2, 2]]), row(1, [[0, 0], [2, 2]]))).toBe(2);
  });

  it("counts half of a curve on each of the two rows it crosses", () => {
    // Row A's edge curves from lane 0 out to lane 3 on the way down to row B.
    const a = row(0, [[0, 3]]);
    const b = row(3, [[3, 3]]);
    expect(laneExtent(a, undefined)).toBe(1.5);
    expect(laneExtent(b, a)).toBe(3);
    // And back in from lane 3 to lane 0.
    const c = row(3, [[3, 0]]);
    expect(laneExtent(c, undefined)).toBe(3);
    expect(laneExtent(row(0, []), c)).toBe(1.5);
  });

  it("ignores the previous row's dangling stubs, which end within that row", () => {
    expect(laneExtent(row(0, []), row(4, [[4, 4]], true))).toBe(0);
  });

  it("starts the message past the furthest lane", () => {
    expect(messageIndent(row(0, [[0, 0]]), undefined)).toBe(LANE_WIDTH + 4);
    expect(messageIndent(row(0, [[0, 0], [2, 2]]), undefined)).toBe(3 * LANE_WIDTH + 4);
  });
});
