import { describe, expect, it } from "vitest";
import { pillsThatFit } from "./pillFit";

describe("fitting a row's ref pills beside its message", () => {
  const widths = [80, 60, 100];

  it("shows every pill when they all fit, with no room kept for the +N pill", () => {
    // 80 + 4 + 60 + 4 + 100
    expect(pillsThatFit(widths, 248, 4, 30)).toBe(3);
    expect(pillsThatFit(widths, 1000, 4, 30)).toBe(3);
  });

  it("leaves room for the +N pill once any pill is left out", () => {
    expect(pillsThatFit(widths, 247, 4, 30)).toBe(2);
    // 80 + 4 + 60 + 4 + 30 exactly
    expect(pillsThatFit(widths, 178, 4, 30)).toBe(2);
    expect(pillsThatFit(widths, 177, 4, 30)).toBe(1);
  });

  it("always shows the first pill, even when it does not fit", () => {
    expect(pillsThatFit(widths, 50, 4, 30)).toBe(1);
    expect(pillsThatFit(widths, 0, 4, 30)).toBe(1);
    expect(pillsThatFit([500], 100, 4, 30)).toBe(1);
  });

  it("can show none, when another pill already comes first", () => {
    expect(pillsThatFit(widths, 50, 4, 30, 0)).toBe(0);
  });

  it("has nothing to fit when there are no pills", () => {
    expect(pillsThatFit([], 0, 4, 30)).toBe(0);
  });
});
