import { describe, expect, it } from "vitest";
import type { RefEntry, RefKind } from "@/lib/bindings";
import { localOnlyNote, remoteBranchesMoved, remoteOnlyNote, tagSyncView, type RemoteTagAnswer } from "@/lib/tagSync";

const local = new Map([
  ["v1", "c1"],
  ["v2", "c2"],
  ["v3", "c3"],
]);

function answers(entries: [string, RemoteTagAnswer][]): Map<string, RemoteTagAnswer> {
  return new Map(entries);
}

describe("tagSyncView", () => {
  it("marks tags every remote has as both, and the rest as local only", () => {
    const view = tagSyncView(
      local,
      answers([
        ["origin", new Map([["v1", "c1"], ["v2", "c2"]])],
        ["backup", new Map([["v1", "c1"]])],
      ]),
      ["origin", "backup"],
    );
    expect(view.local.get("v1")?.state).toBe("both");
    expect(view.local.get("v2")).toEqual({ state: "localOnly", missingOn: ["backup"], elsewhereOn: [] });
    expect(view.local.get("v3")).toEqual({ state: "localOnly", missingOn: ["origin", "backup"], elsewhereOn: [] });
  });

  it("compares only the remotes asked for", () => {
    const view = tagSyncView(local, answers([["origin", new Map([["v2", "c2"]])], ["backup", new Map()]]), ["origin"]);
    expect(view.local.get("v2")?.state).toBe("both");
  });

  it("leaves a tag unknown until the remotes have answered, unless one already lacks it", () => {
    const view = tagSyncView(local, answers([["origin", new Map([["v1", "c1"]])], ["backup", null]]), ["origin", "backup"]);
    expect(view.local.get("v1")?.state).toBe("unknown");
    expect(view.local.get("v2")?.state).toBe("localOnly");
    expect(tagSyncView(local, new Map(), []).local.get("v1")?.state).toBe("unknown");
  });

  it("draws a tag on another commit there twice: local only here, remote only on the remote's commit", () => {
    const view = tagSyncView(local, answers([["origin", new Map([["v1", "c9"], ["v4", "c4"]])]]), ["origin"]);
    expect(view.local.get("v1")).toEqual({ state: "localOnly", missingOn: [], elsewhereOn: ["origin"] });
    expect(view.remoteOnly.get("c9")).toEqual([{ name: "v1", commit: "c9", remotes: ["origin"] }]);
    expect(view.remoteOnly.get("c4")).toEqual([{ name: "v4", commit: "c4", remotes: ["origin"] }]);
    // Only the tag with no name here goes in the Branches panel's list.
    expect(view.notHere.map((t) => t.name)).toEqual(["v4"]);
  });

  it("gathers the remotes that agree on a remote-only tag", () => {
    const view = tagSyncView(new Map(), answers([["origin", new Map([["v5", "c5"]])], ["backup", new Map([["v5", "c5"]])]]), ["origin", "backup"]);
    expect(view.notHere).toEqual([{ name: "v5", commit: "c5", remotes: ["origin", "backup"] }]);
  });
});

describe("tag sync notes", () => {
  it("names the remotes", () => {
    expect(localOnlyNote({ state: "localOnly", missingOn: ["origin", "backup"], elsewhereOn: ["github"] })).toBe(
      "Not on origin and backup. On another commit on github.",
    );
    expect(remoteOnlyNote({ name: "v1", commit: "c", remotes: ["origin"] }, false)).toBe("On origin only; not fetched into this repository.");
  });
});

describe("remoteBranchesMoved", () => {
  function ref(fullName: string, target: string, kind: RefKind): RefEntry {
    return { fullName, target, kind } as RefEntry;
  }
  const remote: RefKind = { type: "remoteBranch", remote: "origin" };
  const before = [
    ref("refs/heads/main", "c1", { type: "localBranch" }),
    ref("refs/remotes/origin/main", "c1", remote),
    ref("refs/tags/v1", "c1", { type: "tag", annotated: true }),
  ];

  it("is false when only local branches or tags change", () => {
    const after = [ref("refs/heads/main", "c2", { type: "localBranch" }), ref("refs/remotes/origin/main", "c1", remote), ref("refs/tags/v2", "c2", { type: "tag", annotated: false })];
    expect(remoteBranchesMoved(before, after)).toBe(false);
  });

  it("is true when a remote-tracking branch moves, appears or goes", () => {
    expect(remoteBranchesMoved(before, [ref("refs/remotes/origin/main", "c2", remote)])).toBe(true);
    expect(remoteBranchesMoved(before, [...before, ref("refs/remotes/origin/dev", "c1", remote)])).toBe(true);
    expect(remoteBranchesMoved(before, before.slice(0, 1))).toBe(true);
  });
});
