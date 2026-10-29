import { beforeEach, describe, expect, it, vi } from "vitest";

const commands = vi.hoisted(() => ({ queryRemoteTag: vi.fn(), opCancel: vi.fn() }));
vi.mock("@/lib/bindings", () => ({ commands }));

const { checkRemoteTags, moveLease, remoteTagLabel, remoteTagNote, remoteTagTickable } = await import("./remoteTags");

const ok = <T>(data: T) => Promise.resolve({ status: "ok", data });
const result = (stderr = "", success = true) => ({ ok: success, exitCode: success ? 0 : 128, stdout: "", stderr, conflicts: false });
const here = "c0ffee00c0ffee00";
const elsewhere = "beefbeefbeefbeef";

beforeEach(() => {
  vi.clearAllMocks();
  commands.opCancel.mockReturnValue(ok(true));
});

describe("asking the remotes whether they have a tag", () => {
  it("reports each remote as it answers: found, missing or failed", async () => {
    commands.queryRemoteTag.mockImplementation((_key: string, _op: string, remote: string) => {
      if (remote === "origin") return ok({ result: result(), tag: { oid: "a1", commit: here } });
      if (remote === "backup") return ok({ result: result(), tag: null });
      return ok({ result: result("ssh: connect to host x port 22: Connection refused\nfatal: Could not read from remote repository.", false), tag: null });
    });
    const states = new Map<string, unknown>();
    checkRemoteTags("repo", "v1", ["origin", "backup", "down"], (remote, state) => states.set(remote, state));
    await vi.waitFor(() => expect(states.size).toBe(3));
    expect(states.get("origin")).toEqual({ state: "found", tag: { oid: "a1", commit: here } });
    expect(states.get("backup")).toEqual({ state: "missing" });
    expect(states.get("down")).toEqual({ state: "failed", message: "Could not read from remote repository." });
  });

  it("stops the checks still running and drops their answers once the dialog closes", async () => {
    let answer: (v: unknown) => void = () => {};
    commands.queryRemoteTag.mockReturnValue(new Promise((resolve) => (answer = resolve)));
    const onResult = vi.fn();
    const stop = checkRemoteTags("repo", "v1", ["origin"], onResult);
    stop();
    expect(commands.opCancel).toHaveBeenCalledWith("repo", commands.queryRemoteTag.mock.calls[0]![1]);
    answer({ status: "ok", data: { result: result(), tag: null } });
    await new Promise((r) => setTimeout(r, 0));
    expect(onResult).not.toHaveBeenCalled();
  });
});

describe("the remote checkboxes in the tag dialogs", () => {
  const found = (commit: string) => ({ state: "found" as const, tag: { oid: "a1", commit } });

  it("say that moving uses --force", () => {
    expect(remoteTagLabel("move", "origin")).toBe("Also move on origin (git push --force)");
    expect(remoteTagLabel("remove", "origin")).toBe("Also remove from origin");
    expect(remoteTagLabel("rename", "origin")).toBe("Also rename on origin");
  });

  it("can be ticked while checking, but not to remove or rename a tag the remote does not have", () => {
    expect(remoteTagTickable("remove", { state: "checking" })).toBe(true);
    expect(remoteTagTickable("remove", { state: "missing" })).toBe(false);
    expect(remoteTagTickable("rename", { state: "missing" })).toBe(false);
    expect(remoteTagTickable("move", { state: "missing" })).toBe(true);
    expect(remoteTagTickable("rename", { state: "failed", message: "x" })).toBe(true);
  });

  it("say what the remote has and what ticking does", () => {
    expect(remoteTagNote("remove", "origin", { state: "checking" }, "v1", here)).toBe("Checking whether origin has v1…");
    expect(remoteTagNote("remove", "origin", found(here), "v1", here)).toBe("origin has v1 on the same commit.");
    expect(remoteTagNote("remove", "origin", found(elsewhere), "v1", here)).toBe("origin has v1 on another commit, beefbee, which is removed too.");
    expect(remoteTagNote("rename", "origin", found(here), "v1", here, "v1.0")).toBe("origin has v1 on the same commit. This pushes v1.0 there, then deletes v1.");
    expect(remoteTagNote("rename", "origin", found(here), "v1", here)).toMatch(/pushes the new name there/);
    expect(remoteTagNote("move", "origin", { state: "missing" }, "v1", here)).toBe("origin does not have v1, so this pushes it there.");
    expect(remoteTagNote("move", "origin", found(here), "v1", here)).toMatch(/^origin has v1 on the same commit\. This replaces it there; .* --force\.$/);
    expect(remoteTagNote("remove", "origin", { state: "failed", message: "Connection refused" }, "v1", here)).toBe("Could not check origin: Connection refused");
  });

  it("lease a move on what was found", () => {
    expect(moveLease({ oid: "a1", commit: here })).toBe("a1");
    expect(moveLease(null)).toBe("");
    expect(moveLease(undefined)).toBeNull();
  });
});
