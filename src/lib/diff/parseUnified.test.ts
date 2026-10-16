import { describe, expect, it } from "vitest";
import { noDiffMessage, parseUnified } from "./parseUnified";

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
