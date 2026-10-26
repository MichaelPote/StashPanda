import { describe, expect, it } from "vitest";
import type { ConfigEntry } from "@/lib/bindings";
import { catalogueSetting, describeValue, otherEntries, parseBool, settingValues } from "./gitSettings";

const entry = (scope: string, key: string, value: string | null): ConfigEntry => ({ scope, origin: `file:${scope}`, key, value });

describe("a setting's value in each file", () => {
  // As Git lists them: keys in lower case, files in the order Git reads them.
  const entries = [
    entry("system", "push.default", "matching"),
    entry("global", "push.followtags", "true"),
    entry("global", "push.default", "current"),
    entry("local", "push.default", "upstream"),
    entry("local", "core.flag", null),
  ];

  it("shows a repository's own value, and what it would inherit from the global and system files", () => {
    expect(settingValues(entries, "push.default", "local")).toEqual({
      own: { scope: "local", value: "upstream" },
      inherited: { scope: "global", value: "current" },
    });
  });

  it("shows the global file's value, inheriting only from the system file", () => {
    expect(settingValues(entries, "push.default", "global")).toEqual({
      own: { scope: "global", value: "current" },
      inherited: { scope: "system", value: "matching" },
    });
  });

  it("compares keys without case, and says when nothing sets a key", () => {
    expect(settingValues(entries, "push.followTags", "local")).toEqual({ own: { scope: null, value: null }, inherited: { scope: "global", value: "true" } });
    expect(settingValues(entries, "rerere.enabled", "local").inherited).toEqual({ scope: null, value: null });
  });

  it("reads a key with no value as true", () => {
    expect(settingValues(entries, "core.flag", "local").own.value).toBe("true");
  });

  it("lists only the keys the catalogue does not explain under Other settings", () => {
    expect(otherEntries(entries, "local").map((e) => e.key)).toEqual(["core.flag"]);
    expect(otherEntries(entries, "global")).toEqual([]);
  });
});

describe("reading values", () => {
  it("accepts every way Git writes a boolean", () => {
    for (const v of ["true", "Yes", "on", "1", ""]) expect(parseBool(v)).toBe(true);
    for (const v of ["false", "NO", "off", "0"]) expect(parseBool(v)).toBe(false);
    expect(parseBool("maybe")).toBeNull();
  });

  it("describes values the way the dialog shows them", () => {
    expect(describeValue(catalogueSetting("PUSH.FOLLOWTAGS"), "yes")).toBe("On");
    expect(describeValue(catalogueSetting("pull.rebase"), "merges")).toBe("Rebase, keeping the branch's own merge commits");
    expect(describeValue(catalogueSetting("user.name"), "Ada")).toBe("Ada");
    expect(describeValue(undefined, "x")).toBe("x");
  });
});
