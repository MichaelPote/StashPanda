import { describe, expect, it } from "vitest";
import { conflictStages, missingVersionNote, shortSideNames, takeLabel } from "./conflicts";

const names = { title: "Merging feature into main", ours: "main", theirs: "feature" };

describe("conflict stages", () => {
  // Each pair of letters against the stages `git ls-files -u` listed for it after a real merge
  // (Git 2.43): rename/rename leaves AU on our new name, UA on theirs and DD on the old one; a file
  // main added in a folder feature renamed leaves AU.
  it("follows the stages Git keeps for every pair of letters", () => {
    expect(conflictStages("UU")).toEqual({ base: true, ours: true, theirs: true });
    expect(conflictStages("AA")).toEqual({ base: false, ours: true, theirs: true });
    expect(conflictStages("UD")).toEqual({ base: true, ours: true, theirs: false });
    expect(conflictStages("DU")).toEqual({ base: true, ours: false, theirs: true });
    expect(conflictStages("AU")).toEqual({ base: false, ours: true, theirs: false });
    expect(conflictStages("UA")).toEqual({ base: false, ours: false, theirs: true });
    expect(conflictStages("DD")).toEqual({ base: true, ours: false, theirs: false });
  });

  it("says why a version is missing, naming the side", () => {
    expect(missingVersionNote("UU", "base", names)).toBeNull();
    expect(missingVersionNote("AA", "base", names)).toBe("Both sides added this file, so there is no base version: the other side shows as wholly added.");
    expect(missingVersionNote("AU", "base", names)).toBe("main added this file, so there is no base version: the other side shows as wholly added.");
    expect(missingVersionNote("AU", "theirs", names)).toBe("feature does not have this file, so that side is empty.");
    expect(missingVersionNote("UA", "ours", names)).toBe("main does not have this file, so that side is empty.");
    expect(missingVersionNote("UD", "theirs", names)).toBe("feature deleted this file, so that side is empty.");
    expect(missingVersionNote("DU", "ours", names)).toBe("main deleted this file, so that side is empty.");
    expect(missingVersionNote("DU", "theirs", names)).toBeNull();
  });
});

describe("short side names for the Take buttons", () => {
  it("keeps what comes after the last slash", () => {
    expect(shortSideNames({ ours: "feature/planting-polygons", theirs: "origin/dependabot/npm_and_yarn/vite-8.3.4" })).toEqual({
      ours: "planting-polygons",
      theirs: "vite-8.3.4",
    });
    expect(shortSideNames({ ours: "main", theirs: "abc1234 from feature/x" })).toEqual({ ours: "main", theirs: "abc1234 from x" });
  });

  it("falls back to the first part, then to the full names, when the two would read the same", () => {
    expect(shortSideNames({ ours: "feature/x", theirs: "origin/feature/x" })).toEqual({ ours: "feature", theirs: "origin" });
    expect(shortSideNames({ ours: "feature/x", theirs: "feature/old/x" })).toEqual({ ours: "feature/x", theirs: "feature/old/x" });
    expect(shortSideNames({ ours: "main", theirs: "stash@{0}" })).toEqual({ ours: "main", theirs: "stash@{0}" });
  });
});

describe("the Take buttons' labels", () => {
  it("names the pane each side is in for the comparison shown", () => {
    expect(takeLabel("ours", "planting-polygons", { from: "ours", to: "theirs" })).toBe("Take planting-polygons (Left)");
    expect(takeLabel("theirs", "vite-8.3.4", { from: "ours", to: "theirs" })).toBe("Take vite-8.3.4 (Right)");
    expect(takeLabel("ours", "main", { from: "base", to: "ours" })).toBe("Take main (Right)");
    expect(takeLabel("theirs", "feature", { from: "base", to: "ours" })).toBe("Take feature");
    expect(takeLabel("theirs", "feature", null)).toBe("Take feature");
  });
});
