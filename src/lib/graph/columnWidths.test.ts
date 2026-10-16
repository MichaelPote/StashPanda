import { describe, expect, it } from "vitest";
import { COLUMN_LIMITS, fitColumns } from "./columnWidths";

describe("fitting the commit table's columns into the room there is", () => {
  const wide = { graph: 0, author: 400, date: 260, id: 355 };

  it("leaves widths that fit as they are", () => {
    const widths = { graph: 60, author: 120, date: 100, id: 60 };
    expect(fitColumns(widths, 1000, 160)).toEqual(widths);
    // Not measured yet.
    expect(fitColumns(wide, 0, 160)).toEqual(wide);
  });

  it("shrinks columns saved too wide, so the message column keeps its minimum", () => {
    const fitted = fitColumns(wide, 785, 160);
    const total = fitted.graph + fitted.author + fitted.date + fitted.id;
    expect(785 - total).toBeGreaterThanOrEqual(160);
    // Each gives up the same share of what it has above its minimum.
    expect(fitted.author).toBeLessThan(400);
    expect(fitted.date).toBeLessThan(260);
    expect(fitted.id).toBeLessThan(355);
    expect(fitted.author).toBeGreaterThan(fitted.id);
  });

  it("never goes below a column's own minimum, even with no room at all", () => {
    const fitted = fitColumns(wide, 200, 160);
    expect(fitted).toEqual({ graph: 0, author: COLUMN_LIMITS.author.min, date: COLUMN_LIMITS.date.min, id: COLUMN_LIMITS.id.min });
  });
});
