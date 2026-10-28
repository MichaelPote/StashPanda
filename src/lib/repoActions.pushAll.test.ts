import { beforeEach, describe, expect, it, vi } from "vitest";
import type { PushAllBranch, PushAllResult } from "@/lib/bindings";

const commands = vi.hoisted(() => ({ opPushAll: vi.fn() }));
const notices = vi.hoisted(() => [] as { message: string; success: boolean }[]);
const sessionMap = vi.hoisted(() => new Map<string, unknown>());

vi.mock("@/lib/bindings", () => ({ commands }));
vi.mock("@tauri-apps/plugin-opener", () => ({ openPath: vi.fn(), revealItemInDir: vi.fn() }));
vi.mock("@/stores/dialogs", () => ({ useDialogsStore: () => ({ open: vi.fn() }) }));
vi.mock("@/stores/library", () => ({ repoDisplayName: vi.fn(), useLibraryStore: () => ({}) }));
vi.mock("@/stores/settings", () => ({ useSettingsStore: () => ({ settings: null }) }));
vi.mock("@/stores/uiState", () => ({ useUiStateStore: () => ({ state: null }) }));
vi.mock("@/stores/sessions", () => ({
  useSessionsStore: () => ({
    sessions: sessionMap,
    notify: (message: string) => notices.push({ message, success: false }),
    notifySuccess: (message: string) => notices.push({ message, success: true }),
    refetchRefsAndStatus: vi.fn(),
  }),
}));

import { pushAllBranches, pushAllOutcome, pushAllReason } from "./repoActions";

const ok = <T>(data: T) => Promise.resolve({ status: "ok", data });
const branch = (name: string, outcome: PushAllBranch["outcome"], detail = ""): PushAllBranch => ({ branch: name, to: `origin/${name}`, outcome, detail });

function session(remotes = [{ name: "origin" }]) {
  const s = {
    key: "repo",
    operation: null,
    status: { branch: "main", detached: false, headOid: "abc" },
    refs: { detached: false, remotes, refs: [] },
  };
  sessionMap.set("repo", s);
  return s as never;
}

beforeEach(() => {
  vi.clearAllMocks();
  notices.length = 0;
});

describe("pushAllOutcome", () => {
  it("names one pushed branch with where it went, and several by name", () => {
    expect(pushAllOutcome({ branches: [branch("dev", "pushed"), branch("main", "upToDate")], failures: [] })).toEqual({
      message: "Pushed dev to origin/dev.",
      failed: false,
    });
    const several = pushAllOutcome({ branches: [branch("a", "pushed"), branch("b", "pushed"), branch("c", "pushed")], failures: [] });
    expect(several.message).toBe("Pushed 3 branches: a, b and c.");
  });

  it("says when there was nothing to push", () => {
    expect(pushAllOutcome({ branches: [branch("main", "upToDate")], failures: [] })).toEqual({ message: "Every tracked branch is up to date on its remote.", failed: false });
    expect(pushAllOutcome({ branches: [], failures: [] }).message).toBe("No branch tracks a remote branch, so there was nothing to push.");
  });

  it("reports rejected and skipped branches after the pushed ones, as a failure", () => {
    const result: PushAllResult = {
      branches: [branch("dev", "pushed"), branch("shared", "rejected", "fetch first"), branch("renamed", "skipped", "Git's push.default does not push it")],
      failures: [],
    };
    expect(pushAllOutcome(result)).toEqual({
      message:
        "Pushed dev to origin/dev. shared was not pushed, because origin/shared has commits it lacks. Pull them first. renamed was skipped: Git's push.default does not push it.",
      failed: true,
    });
  });

  it("reports a remote that failed as a whole with Git's reason", () => {
    const failure = { ok: false, exitCode: 128, stdout: "", stderr: "fatal: unable to access 'https://example.invalid/': Could not resolve host", conflicts: false };
    const outcome = pushAllOutcome({ branches: [branch("dev", "failed")], failures: [{ remote: "origin", result: failure }] });
    expect(outcome.failed).toBe(true);
    expect(outcome.message).toContain("Could not resolve host");
  });
});

describe("pushAllBranches", () => {
  it("pushes and reports success in one notice", async () => {
    commands.opPushAll.mockReturnValue(ok({ branches: [branch("dev", "pushed")], failures: [] }));
    await pushAllBranches(session());
    expect(commands.opPushAll).toHaveBeenCalledWith("repo", expect.any(String));
    expect(notices).toEqual([{ message: "Pushed dev to origin/dev.", success: true }]);
  });

  it("does not start without a remote", async () => {
    const s = session([]);
    expect(pushAllReason(s)).toBe("This repository has no remotes to push to.");
    await pushAllBranches(s);
    expect(commands.opPushAll).not.toHaveBeenCalled();
    expect(notices).toEqual([{ message: "This repository has no remotes to push to.", success: false }]);
  });
});
