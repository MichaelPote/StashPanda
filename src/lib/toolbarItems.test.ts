import { describe, expect, it } from "vitest";
import {
  availableCommands,
  DEFAULT_TOOLBAR,
  DIVIDER,
  isDefaultToolbar,
  moveToolbarItem,
  normaliseToolbar,
  resolveToolbar,
  SPACER,
  TOOLBAR_COMMANDS,
} from "./toolbarItems";

describe("normaliseToolbar", () => {
  it("gives the default layout when nothing is saved", () => {
    expect(normaliseToolbar(null)).toEqual(DEFAULT_TOOLBAR);
    expect(normaliseToolbar(undefined)).toEqual(DEFAULT_TOOLBAR);
  });

  it("keeps an empty saved layout empty", () => {
    expect(normaliseToolbar([])).toEqual([]);
  });

  it("drops ids it does not know", () => {
    expect(normaliseToolbar(["fetch", "teleport", "push"])).toEqual(["fetch", "push"]);
  });

  it("keeps only the first appearance of a command", () => {
    expect(normaliseToolbar(["commit", "fetch", "commit", "fetch"])).toEqual(["commit", "fetch"]);
  });

  it("keeps every divider and spacer", () => {
    expect(normaliseToolbar([DIVIDER, "fetch", DIVIDER, SPACER, SPACER, "push"])).toEqual([DIVIDER, "fetch", DIVIDER, SPACER, SPACER, "push"]);
  });
});

describe("resolveToolbar", () => {
  it("gives each item a key that is unique within the layout", () => {
    const entries = resolveToolbar([DIVIDER, "fetch", DIVIDER, SPACER, "fetch", SPACER]);
    expect(entries.map((e) => e.kind)).toEqual(["divider", "command", "divider", "spacer", "spacer"]);
    expect(new Set(entries.map((e) => e.key)).size).toBe(entries.length);
  });

  it("finds each command in the registry", () => {
    const [entry] = resolveToolbar(["commit"]);
    expect(entry?.kind === "command" && entry.command.primary).toBe(true);
  });
});

describe("the registry", () => {
  it("has a unique id for every command, none of them a layout item's", () => {
    const ids = TOOLBAR_COMMANDS.map((c) => c.id);
    expect(new Set(ids).size).toBe(ids.length);
    expect(ids).not.toContain(DIVIDER);
    expect(ids).not.toContain(SPACER);
  });

  it("knows every id in the default layout", () => {
    expect(normaliseToolbar(DEFAULT_TOOLBAR)).toEqual(DEFAULT_TOOLBAR);
  });
});

describe("availableCommands", () => {
  it("lists the commands not on the toolbar, in registry order", () => {
    const available = availableCommands(DEFAULT_TOOLBAR).map((c) => c.id);
    expect(available).not.toContain("commit");
    expect(available).toContain("settings");
    expect(availableCommands([]).length).toBe(TOOLBAR_COMMANDS.length);
  });
});

describe("moveToolbarItem", () => {
  it("moves an item forwards and backwards", () => {
    expect(moveToolbarItem(["a", "b", "c"], 0, 2)).toEqual(["b", "c", "a"]);
    expect(moveToolbarItem(["a", "b", "c"], 2, 0)).toEqual(["c", "a", "b"]);
  });

  it("leaves the layout unchanged for an index out of range", () => {
    expect(moveToolbarItem(["a", "b"], 0, 2)).toEqual(["a", "b"]);
    expect(moveToolbarItem(["a", "b"], -1, 0)).toEqual(["a", "b"]);
  });
});

describe("isDefaultToolbar", () => {
  it("tells the default layout from a changed one", () => {
    expect(isDefaultToolbar([...DEFAULT_TOOLBAR])).toBe(true);
    expect(isDefaultToolbar(moveToolbarItem(DEFAULT_TOOLBAR, 0, 1))).toBe(false);
    expect(isDefaultToolbar([...DEFAULT_TOOLBAR, SPACER])).toBe(false);
  });
});
