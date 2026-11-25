import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { busyAfter } from "./busyAfter";

beforeEach(() => {
  vi.useFakeTimers();
});
afterEach(() => {
  vi.useRealTimers();
});

describe("busyAfter", () => {
  it("says nothing about work that ends before the delay", () => {
    const set = vi.fn();
    const start = busyAfter(150, set);
    const end = start();
    vi.advanceTimersByTime(100);
    end();
    vi.advanceTimersByTime(500);
    expect(set).not.toHaveBeenCalled();
  });

  it("reports busy once work has lasted the delay, and not busy when it ends", () => {
    const set = vi.fn();
    const start = busyAfter(150, set);
    const end = start();
    vi.advanceTimersByTime(149);
    expect(set).not.toHaveBeenCalled();
    vi.advanceTimersByTime(1);
    expect(set.mock.calls).toEqual([[true]]);
    end();
    expect(set.mock.calls).toEqual([[true], [false]]);
  });

  it("stays busy until the last of several overlapping pieces of work ends", () => {
    const set = vi.fn();
    const start = busyAfter(150, set);
    const endFirst = start();
    vi.advanceTimersByTime(100);
    const endSecond = start();
    vi.advanceTimersByTime(60);
    // 160 ms since work began, though the second piece is only 60 ms old.
    expect(set.mock.calls).toEqual([[true]]);
    endFirst();
    expect(set.mock.calls).toEqual([[true]]);
    endSecond();
    expect(set.mock.calls).toEqual([[true], [false]]);
  });

  it("ignores a piece of work that is ended twice", () => {
    const set = vi.fn();
    const start = busyAfter(150, set);
    const endFirst = start();
    const endSecond = start();
    endFirst();
    endFirst();
    vi.advanceTimersByTime(200);
    expect(set.mock.calls).toEqual([[true]]);
    endSecond();
    expect(set.mock.calls).toEqual([[true], [false]]);
  });

  it("starts the delay again for work that begins after a break", () => {
    const set = vi.fn();
    const start = busyAfter(150, set);
    start()();
    vi.advanceTimersByTime(140);
    const end = start();
    vi.advanceTimersByTime(140);
    expect(set).not.toHaveBeenCalled();
    vi.advanceTimersByTime(10);
    expect(set.mock.calls).toEqual([[true]]);
    end();
  });
});
