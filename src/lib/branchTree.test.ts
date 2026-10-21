import { describe, expect, it } from "vitest";
import { branchTreeRows, nameParts, type BranchTreeRow } from "./branchTree";

const names = ["main", "feature/sms", "release", "feature/email", "fix/login/timeout"];
const describeRows = (rows: BranchTreeRow<string>[]): string[] =>
  rows.map((r) => `${"  ".repeat(r.depth)}${r.kind === "folder" ? `[${r.name} ${r.count}] ${r.prefix}` : r.entry}`);

describe("branchTreeRows", () => {
  it("puts branches in folders by their names, folders first, with full names on the branches", () => {
    const rows = branchTreeRows(names, (n) => n, "local:", () => true);
    expect(describeRows(rows)).toEqual([
      "[feature 2] feature/",
      "  feature/email",
      "  feature/sms",
      "[fix 1] fix/",
      "  [login 1] fix/login/",
      "    fix/login/timeout",
      "main",
      "release",
    ]);
  });

  it("leaves out the rows inside a closed folder, and keys folders by their path", () => {
    const rows = branchTreeRows(names, (n) => n, "local:", (key) => key !== "local:fix/");
    expect(describeRows(rows)).toEqual(["[feature 2] feature/", "  feature/email", "  feature/sms", "[fix 1] fix/", "main", "release"]);
    expect(rows.filter((r) => r.kind === "folder").map((r) => r.key)).toEqual(["local:feature/", "local:fix/"]);
  });

  it("lists nothing for no branches", () => {
    expect(branchTreeRows([], (n: string) => n, "x:", () => true)).toEqual([]);
  });
});

describe("splitting a branch name so its folder parts can shrink first", () => {
  it("keeps each folder part's slash and the last part whole", () => {
    expect(nameParts("dependabot/npm_and_yarn/axios-1.7")).toEqual({ folders: ["dependabot/", "npm_and_yarn/"], last: "axios-1.7" });
    expect(nameParts("main")).toEqual({ folders: [], last: "main" });
  });
});
