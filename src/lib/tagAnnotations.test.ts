import { describe, expect, it, vi } from "vitest";

vi.mock("@/lib/bindings", () => ({ commands: {} }));

const { tagTooltip } = await import("./tagAnnotations");

describe("a tag pill's tooltip", () => {
  const now = Date.UTC(2026, 9, 25, 12) / 1000;

  it("shows an annotated tag's whole message and who tagged it when", () => {
    const tip = tagTooltip("refs/tags/v1.2", "v1.2", {
      annotated: true,
      taggerName: "Ada Okafor",
      taggerEmail: "ada@example.com",
      taggerTime: now - 3 * 86400,
      message: "Release 1.2\n\n- Faster graph",
    }, now * 1000);
    expect(tip.heading).toBe("v1.2");
    expect(tip.content).toMatch(/^Release 1\.2\n\n- Faster graph\n\nTagged by Ada Okafor <ada@example\.com>, 3 days ago \(2026-10-22 \d\d:\d\d\)$/);
  });

  it("says a lightweight tag has no annotation", () => {
    const tip = tagTooltip("refs/tags/v1", "v1", { annotated: false, taggerName: "", taggerEmail: "", taggerTime: 0, message: "" });
    expect(tip.content).toBe("A lightweight tag: it has no message, tagger or date of its own.");
  });

  it("shows the ref name until the annotation has loaded", () => {
    expect(tagTooltip("refs/tags/v1", "v1", undefined)).toEqual({ heading: "v1", content: "refs/tags/v1" });
  });
});
