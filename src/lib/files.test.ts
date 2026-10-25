import { describe, expect, it } from "vitest";
import type { StatusEntry, StatusSnapshot } from "@/lib/bindings";
import { commitFiles, commitPaths, partlyStagedCommitted, worktreeActionPaths, worktreeSections } from "./files";

const entry = (path: string, x: string, y: string, origPath: string | null = null): StatusEntry => ({
  path,
  origPath,
  x,
  y,
  staged: x !== ".",
  unstaged: y !== ".",
  untracked: false,
  ignored: false,
  conflicted: false,
  renameScore: origPath ? 100 : null,
  isSubmodule: false,
});
const snapshot = (...entries: StatusEntry[]) => ({ entries }) as unknown as StatusSnapshot;

describe("renamed and copied files", () => {
  // `git mv old.txt new.txt`, then an edit to new.txt that is not staged.
  const renamedThenEdited = snapshot(entry("new.txt", "R", "M", "old.txt"));
  // copy.txt is a staged copy of src.txt, which has staged and unstaged changes of its own.
  const copied = snapshot(entry("copy.txt", "C", ".", "src.txt"), entry("src.txt", "M", "M"));

  it("gives the original path only to the row whose change is the rename", () => {
    const [, staged, unstaged] = worktreeSections(renamedThenEdited);
    expect(staged.items.map((i) => [i.path, i.oldPath])).toEqual([["new.txt", "old.txt"]]);
    expect(unstaged.items.map((i) => [i.path, i.oldPath])).toEqual([["new.txt", null]]);
  });

  it("acts on both paths of a rename, but only on the new path for its unstaged edit", () => {
    const stagedRow = worktreeActionPaths(renamedThenEdited, new Set(["staged:new.txt"]));
    expect(stagedRow.unstage).toEqual(["new.txt", "old.txt"]);
    expect(stagedRow.discardToHead).toEqual(["new.txt", "old.txt"]);
    const unstagedRow = worktreeActionPaths(renamedThenEdited, new Set(["unstaged:new.txt"]));
    expect(unstagedRow.stage).toEqual(["new.txt"]);
    expect(unstagedRow.discardToIndex).toEqual(["new.txt"]);
  });

  it("keeps a copy's source out of anything done to the copy", () => {
    const [, staged] = worktreeSections(copied);
    expect(staged.items.find((i) => i.path === "copy.txt")?.oldPath).toBe("src.txt");
    const p = worktreeActionPaths(copied, new Set(["staged:copy.txt"]));
    expect(p.unstage).toEqual(["copy.txt"]);
    expect(p.discardToHead).toEqual(["copy.txt"]);
  });

  it("commits both paths of a rename and only the copy itself", () => {
    const status = snapshot(entry("new.txt", "R", ".", "old.txt"), entry("copy.txt", "C", ".", "src.txt"), entry("src.txt", "M", "."), entry("b.txt", ".", "M"));
    const { files } = commitFiles(status, new Set(), "all");
    expect(commitPaths(files, new Set(["new.txt", "copy.txt", "b.txt"]))).toEqual(["b.txt", "copy.txt", "new.txt", "old.txt"]);
  });
});

describe("partly staged files in a commit", () => {
  // a.txt is partly staged, b.txt staged, c.txt changed but not staged.
  const status = snapshot(entry("a.txt", "M", "M"), entry("b.txt", "M", "."), entry("c.txt", ".", "M"));
  const { files } = commitFiles(status, new Set(), "all");
  const committed = (...ticked: string[]) => partlyStagedCommitted(files, new Set(ticked)).map((f) => f.path);

  it("takes only their staged part while every ticked file is staged", () => {
    expect(committed("a.txt", "b.txt")).toEqual([]);
    expect(commitPaths(files, new Set(["a.txt"]))).toEqual(["a.txt"]);
    expect(committed("a.txt")).toEqual([]);
  });

  it("takes them as they are on disk once a ticked file is not staged", () => {
    expect(committed("a.txt", "c.txt")).toEqual(["a.txt"]);
    expect(committed("b.txt", "c.txt")).toEqual([]);
  });
});
