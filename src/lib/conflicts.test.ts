import { describe, expect, it } from "vitest";
import { conflictStages, missingVersionNote } from "./conflicts";

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
