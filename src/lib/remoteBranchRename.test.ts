import { describe, expect, it } from "vitest";
import type { OpResult } from "@/lib/bindings";
import { branchesTracking, listNames, remoteRenameRefusal } from "./remoteBranchRename";

const failed = (stderr: string): OpResult => ({ ok: false, exitCode: 1, stdout: "", stderr, conflicts: false }) as OpResult;

// What Git 2.43 prints for each refusal, captured from a push to a bare repository.
const NEW_NAME_EXISTS = `error: atomic push failed for ref refs/heads/unfetched. status: 7
To /srv/origin.git
 ! [rejected]        (delete) -> feature/y (atomic push failed)
 ! [rejected]        45de341a4e4704ed5fd8641f2264680789def98f -> unfetched (stale info)
error: failed to push some refs to '/srv/origin.git'`;
const OLD_NAME_MOVED = `error: atomic push failed for ref refs/heads/feature/y. status: 7
To /srv/origin.git
 ! [rejected]        (delete) -> feature/y (stale info)
 ! [rejected]        45de341a4e4704ed5fd8641f2264680789def98f -> feature/z (atomic push failed)
error: failed to push some refs to '/srv/origin.git'`;
const DEFAULT_BRANCH = `remote: error: refusing to delete the current branch: refs/heads/main
To /srv/origin.git
 ! [remote rejected] main (deletion of the current branch prohibited)
 ! [remote rejected] 45de341a4e4704ed5fd8641f2264680789def98f -> trunk (atomic push failure)
error: failed to push some refs to '/srv/origin.git'`;

describe("remoteRenameRefusal", () => {
  it("says the new name is taken when the remote already has it", () => {
    const text = remoteRenameRefusal(failed(NEW_NAME_EXISTS), "origin", "feature/y", "unfetched");
    expect(text).toContain("origin already has a branch unfetched");
    expect(text).toContain("Nothing was changed");
  });

  it("says to fetch first when the branch has moved on since the last fetch", () => {
    const text = remoteRenameRefusal(failed(OLD_NAME_MOVED), "origin", "feature/y", "feature/z");
    expect(text).toContain("feature/y on origin has commits that this repository has not fetched");
  });

  it("explains the remote's default branch", () => {
    expect(remoteRenameRefusal(failed(DEFAULT_BRANCH), "origin", "main", "trunk")).toContain("the remote's default branch");
  });

  it("recognises a remote that cannot push atomically, and any other refusal by the remote", () => {
    expect(remoteRenameRefusal(failed("fatal: the receiving end does not support --atomic push"), "origin", "a", "b")).toContain("two changes in one push");
    expect(remoteRenameRefusal(failed(" ! [remote rejected] a (protected branch hook declined)"), "origin", "a", "b")).toContain("protected branch");
  });

  it("leaves anything else to Git's own message", () => {
    expect(remoteRenameRefusal(failed("fatal: unable to access 'https://example.invalid/': Could not resolve host"), "origin", "a", "b")).toBeNull();
  });
});

describe("branchesTracking", () => {
  const ref = (short: string, type: string, upstream: string | null, upstreamGone = false) => ({ kind: { type }, short, upstream, upstreamGone });

  it("lists the local branches that track the remote branch", () => {
    const refs = [
      ref("dev", "localBranch", "refs/remotes/origin/feature/x"),
      ref("other", "localBranch", "refs/remotes/origin/main"),
      ref("elsewhere", "localBranch", "refs/remotes/fork/feature/x"),
      ref("gone", "localBranch", "refs/remotes/origin/feature/x", true),
      ref("origin/feature/x", "remoteBranch", null),
    ];
    expect(branchesTracking(refs, "origin", "feature/x")).toEqual(["dev"]);
  });
});

describe("listNames", () => {
  it("joins names the way a sentence does", () => {
    expect(listNames([])).toBe("");
    expect(listNames(["a"])).toBe("a");
    expect(listNames(["a", "b"])).toBe("a and b");
    expect(listNames(["a", "b", "c"])).toBe("a, b and c");
  });
});
