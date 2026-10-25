import { describe, expect, it } from "vitest";
import { highlightPair, noDiffMessage, parseUnified } from "./parseUnified";

describe("noDiffMessage", () => {
  it("names a change of mode alone, as `git diff` prints it with no hunk", () => {
    const parsed = parseUnified("diff --git a/run.sh b/run.sh\nold mode 100644\nnew mode 100755\n");
    expect(parsed.files[0]).toMatchObject({ oldMode: "100644", newMode: "100755", hunks: [] });
    expect(noDiffMessage(null, parsed)).toBe("Only the file mode changed, from 100644 to 100755. The file is now executable.");
    const back = parseUnified("diff --git a/run.sh b/run.sh\nold mode 100755\nnew mode 100644\n");
    expect(noDiffMessage(null, back)).toBe("Only the file mode changed, from 100755 to 100644. The file is no longer executable.");
  });

  it("says why a nested repository and a submodule with untracked files have no lines", () => {
    const empty = parseUnified("");
    expect(noDiffMessage("nestedRepository", empty)).toContain("Git repository of its own");
    expect(noDiffMessage("submoduleUntracked", empty)).toContain("files it does not track");
  });

  it("falls back to the plain message", () => {
    expect(noDiffMessage(null, parseUnified(""))).toBe("No differences to show.");
    expect(noDiffMessage(null, null)).toBe("No differences to show.");
  });
});

describe("highlightPair", () => {
  it("marks changed words separated only by whitespace as one run", () => {
    const { old, new: next } = highlightPair("# stays on the upload routes", "# stops reading request fields on the upload routes");
    expect(old).toEqual([
      { text: "# ", changed: false },
      { text: "stays", changed: true },
      { text: " on the upload routes", changed: false },
    ]);
    expect(next).toEqual([
      { text: "# ", changed: false },
      { text: "stops reading request fields", changed: true },
      { text: " on the upload routes", changed: false },
    ]);
  });

  it("reports where words were removed with nothing put in their place", () => {
    expect(highlightPair("a b c", "a c").removedAt).toEqual([2]);
    expect(highlightPair("call(x, y)", "call(x,").removedAt).toEqual([7]);
    expect(highlightPair("one two three", "one").removedAt).toEqual([3]);
  });

  it("reports no removal where the removed words were replaced", () => {
    expect(highlightPair("a b c", "a x c").removedAt).toEqual([]);
  });

  it("marks only the whitespace added next to punctuation", () => {
    const { old, new: next } = highlightPair("defaultValue: false})", "defaultValue: false })");
    expect(old).toEqual([{ text: "defaultValue: false})", changed: false }]);
    expect(next).toEqual([
      { text: "defaultValue: false", changed: false },
      { text: " ", changed: true },
      { text: "})", changed: false },
    ]);
  });

  it("marks a whitespace-only change between unchanged words, and a removed space's place", () => {
    const { new: next } = highlightPair("a b", "a  b");
    expect(next).toEqual([
      { text: "a ", changed: false },
      { text: " ", changed: true },
      { text: "b", changed: false },
    ]);
    expect(highlightPair("x = 1", "x =1").removedAt).toEqual([3]);
  });

  it("leaves a whitespace-only change unmarked when whitespace is ignored", () => {
    expect(highlightPair("a b", "a  b", true).new).toEqual([{ text: "a  b", changed: false }]);
    expect(highlightPair("false})", "false })", true).new).toEqual([{ text: "false })", changed: false }]);
    expect(highlightPair("x = 1", "x =1", true).removedAt).toEqual([]);
  });
});
