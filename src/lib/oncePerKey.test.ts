import { describe, expect, it, vi } from "vitest";
import { oncePerKey } from "./oncePerKey";

/** A task that settles only when the test says so. */
function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

describe("oncePerKey", () => {
  it("starts one task for two calls with the same key, and gives both its result", async () => {
    const once = oncePerKey<object>();
    const open = deferred<object>();
    const task = vi.fn(() => open.promise);

    const first = once("repo", task);
    const second = once("repo", task);
    expect(task).toHaveBeenCalledTimes(1);

    const session = {};
    open.resolve(session);
    expect(await first).toBe(session);
    expect(await second).toBe(session);
  });

  it("runs tasks for different keys side by side", () => {
    const once = oncePerKey<number>();
    const task = vi.fn(() => deferred<number>().promise);
    void once("a", task);
    void once("b", task);
    expect(task).toHaveBeenCalledTimes(2);
  });

  it("starts a new task once the earlier one has finished", async () => {
    const once = oncePerKey<number>();
    const task = vi.fn().mockResolvedValueOnce(1).mockResolvedValueOnce(2);
    expect(await once("repo", task)).toBe(1);
    expect(await once("repo", task)).toBe(2);
    expect(task).toHaveBeenCalledTimes(2);
  });

  it("gives both callers the failure, and lets the next call try again", async () => {
    const once = oncePerKey<number>();
    const open = deferred<number>();
    const first = once("repo", () => open.promise);
    const second = once("repo", () => Promise.resolve(0));
    open.reject(new Error("no such repository"));
    await expect(first).rejects.toThrow("no such repository");
    await expect(second).rejects.toThrow("no such repository");
    expect(await once("repo", () => Promise.resolve(3))).toBe(3);
  });
});
