import { beforeEach, describe, expect, it, vi } from "vitest";
import type { PushTarget, SyncResult } from "@/lib/bindings";

const commands = vi.hoisted(() => ({
  getPushTarget: vi.fn(),
  getUpstream: vi.fn(),
  getPullMode: vi.fn(),
  opPush: vi.fn(),
  opSync: vi.fn(),
  opSetPushDefault: vi.fn(),
  opBranchSetUpstream: vi.fn(),
  opRemoveStaleLock: vi.fn(),
}));
const notices = vi.hoisted(() => [] as { message: string; action: { label: string } | null }[]);
const dialogs = vi.hoisted(() => ({ open: vi.fn() }));
const sessionMap = vi.hoisted(() => new Map<string, unknown>());

vi.mock("@/lib/bindings", () => ({ commands }));
vi.mock("@tauri-apps/plugin-opener", () => ({ openPath: vi.fn(), revealItemInDir: vi.fn() }));
vi.mock("@/stores/dialogs", () => ({ useDialogsStore: () => dialogs }));
vi.mock("@/stores/library", () => ({ repoDisplayName: vi.fn(), useLibraryStore: () => ({}) }));
vi.mock("@/stores/settings", () => ({ useSettingsStore: () => ({ settings: null }) }));
vi.mock("@/stores/uiState", () => ({ useUiStateStore: () => ({ state: null }) }));
vi.mock("@/stores/sessions", () => ({
  useSessionsStore: () => ({
    sessions: sessionMap,
    notify: (message: string, action: { label: string } | null = null) => notices.push({ message, action }),
    notifySuccess: (message: string) => notices.push({ message, action: null }),
    refetchRefsAndStatus: vi.fn(),
  }),
}));

import { pushBranch, pushCurrentBranch, syncCurrentBranch, syncOutcome } from "./repoActions";

const ok = <T>(data: T) => Promise.resolve({ status: "ok", data });
const pushed = { ok: true, exitCode: 0, stdout: "", stderr: "", conflicts: false };
const refusal = {
  status: "error",
  error: { kind: "invalidPath", path: "feature", reason: "it tracks origin/main, a branch with another name, and Git's push.default (simple) does not push to a branch with another name; choose where to push it in the Push dialog" },
};

/** A clean working tree on `feature`, which tracks `origin/main`. */
function session() {
  const s = {
    key: "repo",
    info: { isBare: false, displayName: "repo" },
    operation: null,
    lastFetchAt: 0,
    status: { branch: "feature", detached: false, headOid: "abc", inProgress: null, conflictedCount: 0, stagedCount: 0, unstagedCount: 0 },
    refs: {
      detached: false,
      remotes: [{ name: "origin" }],
      refs: [{ kind: { type: "localBranch" }, short: "feature", upstream: "refs/remotes/origin/main" }],
    },
  };
  sessionMap.set("repo", s);
  return s as never;
}

beforeEach(() => {
  vi.clearAllMocks();
  notices.length = 0;
  commands.getUpstream.mockReturnValue(ok({ remote: "origin", branch: "main" }));
  commands.getPullMode.mockReturnValue(ok(null));
  commands.opPush.mockReturnValue(ok(pushed));
  commands.opSetPushDefault.mockReturnValue(ok(pushed));
  commands.opBranchSetUpstream.mockReturnValue(ok(pushed));
  commands.opRemoveStaleLock.mockReturnValue(ok(null));
});

describe("pushing a branch to a new remote branch", () => {
  const configLocked = { status: "error", error: { kind: "configLocked", lockPath: ".git/config.lock" } };
  /** `feature` tracking nothing, and pushed through the Push dialog to origin/feature with tracking on. */
  function untracked() {
    const s = session() as unknown as { refs: { refs: { upstream: string | null }[] } };
    s.refs.refs[0]!.upstream = null;
    commands.getPushTarget.mockReturnValue(ok(null));
    dialogs.open.mockResolvedValueOnce({ target: { remote: "origin", branch: "feature" }, setUpstream: true });
    return s as never;
  }

  it("pushes, then sets the upstream as a step of its own", async () => {
    await pushCurrentBranch(untracked());
    expect(commands.opPush).toHaveBeenCalledWith("repo", expect.any(String), "feature", { remote: "origin", branch: "feature" });
    expect(commands.opBranchSetUpstream).toHaveBeenCalledWith("repo", "feature", "refs/remotes/origin/feature");
    expect(commands.opPush.mock.invocationCallOrder[0]).toBeLessThan(commands.opBranchSetUpstream.mock.invocationCallOrder[0]!);
    expect(notices.map((n) => n.message)).toEqual(["Pushed feature to origin/feature. feature now tracks origin/feature."]);
  });

  it("offers to remove a config.lock left behind, then sets the upstream", async () => {
    const s = untracked();
    commands.opBranchSetUpstream.mockReturnValueOnce(Promise.resolve(configLocked));
    dialogs.open.mockResolvedValueOnce(true);
    await pushCurrentBranch(s);
    expect(dialogs.open).toHaveBeenLastCalledWith("confirm", expect.objectContaining({ title: "Remove Git's lock file?" }));
    expect(commands.opRemoveStaleLock).toHaveBeenCalledWith("repo", "config");
    expect(commands.opBranchSetUpstream).toHaveBeenCalledTimes(2);
    expect(notices.map((n) => n.message)).toEqual(["Pushed feature to origin/feature. feature now tracks origin/feature."]);
  });

  it("says the push worked but the branch does not track it yet when the lock is kept", async () => {
    const s = untracked();
    commands.opBranchSetUpstream.mockReturnValueOnce(Promise.resolve(configLocked));
    dialogs.open.mockResolvedValueOnce(false);
    await pushCurrentBranch(s);
    expect(commands.opRemoveStaleLock).not.toHaveBeenCalled();
    expect(notices).toHaveLength(1);
    expect(notices[0]!.message).toMatch(/^Pushed feature to origin\/feature\. But feature does not track origin\/feature yet\. .*\.git\/config\.lock/);
    expect(notices[0]!.action?.label).toBe("Track Remote Branch…");
  });

  it("pushes a branch that is not checked out", async () => {
    const s = session() as unknown as { refs: { refs: { kind: { type: string }; short: string; upstream: string | null }[] } };
    s.refs.refs.push({ kind: { type: "localBranch" }, short: "hotfix/x", upstream: null });
    commands.getPushTarget.mockReturnValue(ok(null));
    dialogs.open.mockResolvedValueOnce({ target: { remote: "origin", branch: "hotfix/x" }, setUpstream: true });
    await pushBranch(s as never, "hotfix/x");
    expect(dialogs.open).toHaveBeenCalledWith("push", expect.objectContaining({ branch: "hotfix/x", tracks: null }));
    expect(commands.opPush).toHaveBeenCalledWith("repo", expect.any(String), "hotfix/x", { remote: "origin", branch: "hotfix/x" });
    expect(commands.opBranchSetUpstream).toHaveBeenCalledWith("repo", "hotfix/x", "refs/remotes/origin/hotfix/x");
  });
});

describe("pushing without choosing a target", () => {
  it("leaves the target to the backend, which applies push.default", async () => {
    commands.getPushTarget.mockReturnValue(ok({ remote: "origin", branch: "feature" }));
    await pushCurrentBranch(session());
    expect(dialogs.open).not.toHaveBeenCalled();
    expect(commands.opPush).toHaveBeenCalledWith("repo", expect.any(String), "feature", null);
    expect(notices.map((n) => n.message)).toEqual(["Pushed feature to origin/feature."]);
  });

  it("asks where to push when the upstream has another name, and can push there this time only", async () => {
    commands.getPushTarget.mockReturnValue(Promise.resolve(refusal));
    dialogs.open.mockResolvedValue({ kind: "once" });
    await pushCurrentBranch(session());
    expect(dialogs.open).toHaveBeenCalledWith("pushDefault", { key: "repo", branch: "feature", upstream: { remote: "origin", branch: "main" }, action: "push" });
    expect(commands.opSetPushDefault).not.toHaveBeenCalled();
    expect(commands.opPush).toHaveBeenCalledWith("repo", expect.any(String), "feature", { remote: "origin", branch: "main" });
    expect(notices.map((n) => n.message)).toEqual(["Pushed feature to origin/main."]);
  });

  it("saves push.default for the repository and then leaves the target to Git", async () => {
    commands.getPushTarget.mockReturnValue(Promise.resolve(refusal));
    dialogs.open.mockResolvedValue({ kind: "always", mode: "current" });
    await pushCurrentBranch(session());
    expect(commands.opSetPushDefault).toHaveBeenCalledWith("repo", "current");
    expect(commands.opPush).toHaveBeenCalledWith("repo", expect.any(String), "feature", null);
    expect(notices.map((n) => n.message)).toEqual(["Pushed feature to origin/feature."]);
  });

  it("pushes nothing when the setting cannot be saved", async () => {
    commands.getPushTarget.mockReturnValue(Promise.resolve(refusal));
    dialogs.open.mockResolvedValue({ kind: "always", mode: "upstream" });
    commands.opSetPushDefault.mockReturnValue(ok({ ...pushed, ok: false, exitCode: 255, stderr: "error: could not lock config file .git/config" }));
    await pushCurrentBranch(session());
    expect(commands.opPush).not.toHaveBeenCalled();
    expect(notices.map((n) => n.message)).toEqual(["The setting could not be saved: error: could not lock config file .git/config"]);
  });

  it("opens the Push dialog to choose another branch", async () => {
    commands.getPushTarget.mockReturnValue(Promise.resolve(refusal));
    const chosen: PushTarget = { remote: "origin", branch: "feature" };
    dialogs.open.mockResolvedValueOnce({ kind: "otherTarget" }).mockResolvedValueOnce({ target: chosen, setUpstream: false });
    await pushCurrentBranch(session());
    expect(dialogs.open).toHaveBeenLastCalledWith("push", { key: "repo", branch: "feature", remotes: ["origin"], tracks: "origin/main" });
    expect(commands.opPush).toHaveBeenCalledWith("repo", expect.any(String), "feature", chosen);
  });

  it("pushes nothing when that dialog is cancelled", async () => {
    commands.getPushTarget.mockReturnValue(Promise.resolve(refusal));
    dialogs.open.mockResolvedValue(null);
    await pushCurrentBranch(session());
    expect(commands.opSetPushDefault).not.toHaveBeenCalled();
    expect(commands.opPush).not.toHaveBeenCalled();
  });
});

describe("syncing a branch whose upstream has another name", () => {
  const synced: SyncResult = {
    target: { remote: "origin", branch: "main" },
    pushTarget: { remote: "origin", branch: "main" },
    pull: { ok: true, exitCode: 0, stdout: "Already up to date.", stderr: "", conflicts: false },
    push: pushed,
    nothingToPush: false,
    pushStopped: false,
  };

  it("asks before pulling, and can push to the upstream this time only", async () => {
    commands.getPushTarget.mockReturnValue(Promise.resolve(refusal));
    commands.opSync.mockReturnValue(ok(synced));
    dialogs.open.mockResolvedValue({ kind: "once" });
    await syncCurrentBranch(session());
    expect(dialogs.open).toHaveBeenCalledWith("pushDefault", { key: "repo", branch: "feature", upstream: { remote: "origin", branch: "main" }, action: "sync" });
    expect(commands.opSync).toHaveBeenCalledWith("repo", expect.any(String), "feature", expect.anything(), false, { remote: "origin", branch: "main" });
  });

  it("saves push.default and then leaves the push to Git", async () => {
    commands.getPushTarget.mockReturnValue(Promise.resolve(refusal));
    commands.opSync.mockReturnValue(ok(synced));
    dialogs.open.mockResolvedValue({ kind: "always", mode: "upstream" });
    await syncCurrentBranch(session());
    expect(commands.opSetPushDefault).toHaveBeenCalledWith("repo", "upstream");
    expect(commands.opSync).toHaveBeenCalledWith("repo", expect.any(String), "feature", expect.anything(), false, null);
  });

  it("pulls nothing when the dialog is cancelled", async () => {
    commands.getPushTarget.mockReturnValue(Promise.resolve(refusal));
    dialogs.open.mockResolvedValue(null);
    await syncCurrentBranch(session());
    expect(commands.opSync).not.toHaveBeenCalled();
    expect(notices).toEqual([]);
  });

  it("names where the push went, not the upstream it pulled from", () => {
    const result: SyncResult = {
      target: { remote: "origin", branch: "main" },
      pushTarget: { remote: "origin", branch: "feature" },
      pull: { ok: true, exitCode: 0, stdout: "Already up to date.", stderr: "", conflicts: false },
      push: pushed,
      nothingToPush: false,
      pushStopped: false,
    };
    expect(syncOutcome("repo", result, "feature", "merge").message).toBe("Pushed feature to origin/feature.");
  });
});
