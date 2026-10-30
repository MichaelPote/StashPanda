import { describe, expect, it } from "vitest";
import type { RefEntry } from "./bindings";
import { withTrackedBranches } from "./trackedBranches";

const local = (short: string, upstream: string | null, upstreamGone = false): RefEntry =>
  ({ fullName: `refs/heads/${short}`, short, kind: { type: "localBranch" }, upstream, upstreamGone }) as RefEntry;
const remote = (short: string): RefEntry =>
  ({ fullName: `refs/remotes/${short}`, short, kind: { type: "remoteBranch", remote: short.split("/")[0] }, upstream: null, upstreamGone: false }) as RefEntry;

const refs = [
  local("dev", "refs/remotes/origin/dev"),
  local("dev-copy", "refs/remotes/origin/dev"),
  local("solo", null),
  local("old", "refs/remotes/origin/old", true),
  remote("origin/dev"),
  remote("origin/main"),
];

describe("withTrackedBranches", () => {
  it("adds the remote branch a local branch tracks", () => {
    expect(withTrackedBranches(refs, ["refs/heads/dev"]).sort()).toEqual(["refs/heads/dev", "refs/remotes/origin/dev"]);
  });

  it("adds every local branch that tracks a remote branch, but not their other partners", () => {
    expect(withTrackedBranches(refs, ["refs/remotes/origin/dev"]).sort()).toEqual([
      "refs/heads/dev",
      "refs/heads/dev-copy",
      "refs/remotes/origin/dev",
    ]);
  });

  it("adds nothing for a branch that tracks nothing, or whose upstream is gone", () => {
    expect(withTrackedBranches(refs, ["refs/heads/solo", "refs/heads/old", "refs/remotes/origin/main"]).sort()).toEqual([
      "refs/heads/old",
      "refs/heads/solo",
      "refs/remotes/origin/main",
    ]);
  });
});
