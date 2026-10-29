import { beforeEach, describe, expect, it, vi } from "vitest";
import { reactive, watchEffect } from "vue";
import type { PushTarget, SyncResult } from "@/lib/bindings";

const commands = vi.hoisted(() => ({
  getPushTarget: vi.fn(),
  getRewrotePushedCommits: vi.fn(),
  getUpstream: vi.fn(),
  getPullMode: vi.fn(),
  opPush: vi.fn(),
  opPushTo: vi.fn(),
  opDeleteRemoteBranch: vi.fn(),
  opMerge: vi.fn(),
  opBranchFastForwardToUpstream: vi.fn(),
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

import {
  busyReason,
  fastForwardToUpstream,
  mergeInto,
  mergeOutcome,
  mergeRebaseReason,
  gitReason,
  opFailureText,
  pullOutcome,
  pushBranch,
  pushBranchTo,
  pushCurrentBranch,
  renameBranch,
  renameOnRemote,
  sshDeniedDestination,
  sshDiagnoseAction,
  syncCurrentBranch,
  syncOutcome,
  updateRefsLeftBehind,
  upstreamFastForward,
} from "./repoActions";

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
    rewrotePushed: false,
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
      rewrotePushed: false,
    };
    expect(syncOutcome("repo", result, "feature", "merge").message).toBe("Pushed feature to origin/feature.");
  });
});

describe("pushes that git push would send elsewhere, or that were refused", () => {
  const rejected = { ok: false, exitCode: 1, stdout: "", stderr: " ! [rejected]        feature -> feature (non-fast-forward)", conflicts: false };

  it("does not push where Git cannot, and offers the Push dialog instead", async () => {
    commands.getPushTarget.mockReturnValue(
      Promise.resolve({ status: "error", error: { kind: "pushTargetRefused", branch: "feature", reason: "the repository's remote.origin.push setting sends it to refs/for/main, which is not a branch, and StashPanda only pushes to branches" } }),
    );
    await pushCurrentBranch(session());
    expect(commands.opPush).not.toHaveBeenCalled();
    expect(dialogs.open).not.toHaveBeenCalled();
    expect(notices).toHaveLength(1);
    expect(notices[0]!.message).toMatch(/refs\/for\/main/);
    expect(notices[0]!.action?.label).toBe("Choose where…");
  });

  it("says a refusal caused by rewriting the branch here needs a force push, not a pull", async () => {
    commands.getPushTarget.mockReturnValue(ok({ remote: "origin", branch: "feature" }));
    commands.opPush.mockReturnValue(ok(rejected));
    commands.getRewrotePushedCommits.mockReturnValue(ok(true));
    await pushCurrentBranch(session());
    expect(commands.getRewrotePushedCommits).toHaveBeenCalledWith("repo", "feature", { remote: "origin", branch: "feature" });
    expect(notices[0]!.message).toMatch(/rewritten here by an amend or a rebase/);
    expect(notices[0]!.action?.label).toBe("Force push…");
  });

  it("offers to pull when someone else pushed", async () => {
    commands.getPushTarget.mockReturnValue(ok({ remote: "origin", branch: "feature" }));
    commands.opPush.mockReturnValue(ok(rejected));
    commands.getRewrotePushedCommits.mockReturnValue(ok(false));
    await pushCurrentBranch(session());
    expect(notices[0]!.message).toBe("origin/feature has commits that feature does not have. Pull them first, then push again.");
    expect(notices[0]!.action?.label).toBe("Pull…");
  });

  it("offers no pull into the checked-out branch for a branch that is not checked out", async () => {
    const s = session() as unknown as { refs: { refs: { kind: { type: string }; short: string; upstream: string | null }[] } };
    s.refs.refs.push({ kind: { type: "localBranch" }, short: "hotfix/x", upstream: "refs/remotes/origin/hotfix/x" });
    commands.getPushTarget.mockReturnValue(ok({ remote: "origin", branch: "hotfix/x" }));
    commands.opPush.mockReturnValue(ok(rejected));
    commands.getRewrotePushedCommits.mockReturnValue(ok(false));
    await pushBranch(s as never, "hotfix/x");
    expect(notices[0]!.message).toMatch(/Check out hotfix\/x, pull origin\/hotfix\/x into it/);
    expect(notices[0]!.action).toBeNull();
  });

  it("offers a force push, not another sync, after the sync's own rebase rewrote pushed commits", () => {
    const result: SyncResult = {
      target: { remote: "origin", branch: "main" },
      pushTarget: { remote: "origin", branch: "feature" },
      pull: { ok: true, exitCode: 0, stdout: "Successfully rebased and updated refs/heads/feature.", stderr: "", conflicts: false },
      push: rejected,
      nothingToPush: false,
      pushStopped: false,
      rewrotePushed: true,
    };
    const outcome = syncOutcome("repo", result, "feature", "rebase");
    expect(outcome.message).toMatch(/rewrote commits that origin\/feature already had/);
    expect(outcome.action?.label).toBe("Force push…");
  });
});

describe("Push To", () => {
  /** feature (checked out, tracking origin/main), release and topic locally; origin has main and release. */
  function withBranches() {
    const s = session() as unknown as { refs: { refs: { kind: { type: string; remote?: string }; short: string; upstream: string | null; target?: string }[] } };
    s.refs.refs.push(
      { kind: { type: "localBranch" }, short: "release", upstream: null, target: "local-release" },
      { kind: { type: "localBranch" }, short: "topic", upstream: null, target: "local-topic" },
      { kind: { type: "remoteBranch", remote: "origin" }, short: "origin/HEAD", upstream: null, target: "shown-main" },
      { kind: { type: "remoteBranch", remote: "origin" }, short: "origin/main", upstream: null, target: "shown-main" },
      { kind: { type: "remoteBranch", remote: "origin" }, short: "origin/release", upstream: null, target: "shown-release" },
    );
    return s;
  }

  it("offers this repository's other branches and each remote's branches, starting on the upstream", async () => {
    withBranches();
    dialogs.open.mockResolvedValue(null);
    await pushBranchTo("repo", "topic");
    expect(dialogs.open).toHaveBeenCalledWith("pushTo", {
      key: "repo",
      branch: "topic",
      sourceOid: "local-topic",
      servers: [
        // Neither topic itself nor the checked-out feature.
        { remote: null, label: "This repository (local)", branches: [{ name: "release", oid: "local-release" }] },
        {
          remote: "origin",
          label: "origin",
          branches: [
            { name: "main", oid: "shown-main" },
            { name: "release", oid: "shown-release" },
          ],
        },
      ],
      server: 1,
      target: "topic",
      force: false,
      upstream: null,
    });
    expect(commands.opPushTo).not.toHaveBeenCalled();
  });

  it("pushes into a local branch, and offers a force push when Git refuses", async () => {
    withBranches();
    dialogs.open.mockResolvedValue({ remote: null, target: "release", force: false, expected: "local-release", source: "local-topic" });
    commands.opPushTo.mockReturnValue(ok({ ok: false, exitCode: 1, stdout: "", stderr: " ! [rejected]        topic -> release  (non-fast-forward)", conflicts: false }));
    await pushBranchTo("repo", "topic");
    expect(commands.opPushTo).toHaveBeenCalledWith("repo", expect.any(String), "topic", null, "release", false, "local-release", "local-topic");
    expect(notices[0]!.message).toBe("release has commits that topic does not have, so it was not moved: only a fast-forward goes through without Force push.");
    expect(notices[0]!.action?.label).toBe("Force push…");
  });

  it("force-pushes to a remote branch, and says so when the lease refuses", async () => {
    withBranches();
    // The commit the dialog listed the dropped commits for, which the lease then expects.
    dialogs.open.mockResolvedValue({ remote: "origin", target: "release", force: true, expected: "listed-release", source: "listed-topic" });
    commands.opPushTo.mockReturnValueOnce(ok(pushed));
    await pushBranchTo("repo", "topic");
    // Both ends pinned to the commits the dialog listed against.
    expect(commands.opPushTo).toHaveBeenCalledWith("repo", expect.any(String), "topic", "origin", "release", true, "listed-release", "listed-topic");
    expect(notices[notices.length - 1]!.message).toBe("Force-pushed topic to origin/release.");
    commands.opPushTo.mockReturnValueOnce(ok({ ok: false, exitCode: 1, stdout: "", stderr: " ! [rejected]        topic -> release (stale info)", conflicts: false }));
    await pushBranchTo("repo", "topic");
    expect(notices[notices.length - 1]!.message).toMatch(/has moved since it was shown here, so the force push was refused/);
    commands.opPushTo.mockReturnValueOnce(ok({ ok: false, exitCode: null, stdout: "", stderr: "topic has moved since the list of dropped commits was made (source moved), so nothing was pushed.", conflicts: false }));
    await pushBranchTo("repo", "topic");
    expect(notices[notices.length - 1]!.message).toMatch(/^topic has moved since the list of dropped commits was made, so nothing was pushed/);
  });

  it("pushes to a new branch on a remote, with nothing to force and no lease", async () => {
    withBranches();
    dialogs.open.mockResolvedValue({ remote: "origin", target: "topic", force: false, expected: null, source: "local-topic", track: false });
    commands.opPushTo.mockReturnValueOnce(ok(pushed));
    await pushBranchTo("repo", "topic");
    expect(commands.opPushTo).toHaveBeenCalledWith("repo", expect.any(String), "topic", "origin", "topic", false, null, "local-topic");
    expect(commands.opBranchSetUpstream).not.toHaveBeenCalled();
    expect(notices[notices.length - 1]!.message).toBe("Pushed topic to origin/topic.");
  });

  it("makes the branch track where it was pushed, after the push, when asked", async () => {
    withBranches();
    dialogs.open.mockResolvedValue({ remote: "origin", target: "topic", force: false, expected: null, source: "local-topic", track: true });
    commands.opPushTo.mockReturnValueOnce(ok(pushed));
    await pushBranchTo("repo", "topic");
    expect(commands.opBranchSetUpstream).toHaveBeenCalledWith("repo", "topic", "refs/remotes/origin/topic");
    expect(commands.opPushTo.mock.invocationCallOrder[0]).toBeLessThan(commands.opBranchSetUpstream.mock.invocationCallOrder[0]!);
    expect(notices.map((n) => n.message)).toEqual(["Pushed topic to origin/topic. topic now tracks origin/topic."]);
  });

  it("tracks nothing when the push fails", async () => {
    withBranches();
    dialogs.open.mockResolvedValue({ remote: "origin", target: "release", force: false, expected: "shown-release", source: "local-topic", track: true });
    commands.opPushTo.mockReturnValueOnce(ok({ ok: false, exitCode: 1, stdout: "", stderr: "fatal: unable to access", conflicts: false }));
    await pushBranchTo("repo", "topic");
    expect(commands.opBranchSetUpstream).not.toHaveBeenCalled();
  });

  it("starts with Force push ticked when a notice asked for it", async () => {
    withBranches();
    dialogs.open.mockResolvedValue(null);
    await pushBranchTo("repo", "topic", { remote: "origin", target: "release", force: true });
    expect(dialogs.open).toHaveBeenCalledWith("pushTo", expect.objectContaining({ server: 1, target: "release", force: true }));
  });
});

describe("renaming a branch on its remote too", () => {
  const failed = (stderr: string) => ok({ ok: false, exitCode: 1, stdout: "", stderr, conflicts: false });
  beforeEach(() => {
    session();
    commands.opDeleteRemoteBranch.mockReturnValue(ok(pushed));
  });

  it("is offered only for a branch that tracks a remote branch of its own name", async () => {
    const s = session() as unknown as { refs: { refs: { kind: { type: string }; short: string; upstream: string | null; upstreamGone?: boolean }[] } };
    s.refs.refs.push(
      { kind: { type: "localBranch" }, short: "main", upstream: "refs/remotes/origin/main" },
      { kind: { type: "localBranch" }, short: "old", upstream: "refs/remotes/origin/old", upstreamGone: true },
    );
    dialogs.open.mockResolvedValue(null);
    await renameBranch("repo", "main");
    expect(dialogs.open).toHaveBeenLastCalledWith("renameBranch", { key: "repo", name: "main", remoteCopy: { remote: "origin", branch: "main" } });
    // feature tracks origin/main, a branch of another name.
    await renameBranch("repo", "feature");
    expect(dialogs.open).toHaveBeenLastCalledWith("renameBranch", { key: "repo", name: "feature", remoteCopy: null });
    await renameBranch("repo", "old");
    expect(dialogs.open).toHaveBeenLastCalledWith("renameBranch", { key: "repo", name: "old", remoteCopy: null });
    expect(commands.opPush).not.toHaveBeenCalled();
  });

  it("pushes the new name, tracks it, then deletes the old name, in that order", async () => {
    await renameOnRemote("repo", "main", "dev", "origin");
    expect(commands.opPush).toHaveBeenCalledWith("repo", expect.any(String), "dev", { remote: "origin", branch: "dev" });
    expect(commands.opBranchSetUpstream).toHaveBeenCalledWith("repo", "dev", "refs/remotes/origin/dev");
    expect(commands.opDeleteRemoteBranch).toHaveBeenCalledWith("repo", expect.any(String), "origin", "main");
    const order = [commands.opPush, commands.opBranchSetUpstream, commands.opDeleteRemoteBranch].map((f) => f.mock.invocationCallOrder[0]!);
    expect(order).toEqual([...order].sort((a, b) => a - b));
    expect(notices.map((n) => n.message)).toEqual(["Renamed branch main to dev, here and on origin. dev now tracks origin/dev."]);
  });

  it("deletes nothing when the push fails", async () => {
    commands.opPush.mockReturnValueOnce(failed(" ! [rejected]        dev -> dev (fetch first)"));
    await renameOnRemote("repo", "main", "dev", "origin");
    expect(commands.opBranchSetUpstream).not.toHaveBeenCalled();
    expect(commands.opDeleteRemoteBranch).not.toHaveBeenCalled();
    expect(notices[0]!.message).toBe(
      "Renamed branch main to dev. origin already has a branch dev with commits that dev does not have, so the push was refused. origin/main is unchanged, and dev still tracks it.",
    );
  });

  it("deletes nothing when the branch cannot be made to track the new name", async () => {
    commands.opBranchSetUpstream.mockReturnValueOnce(failed("fatal: could not lock config file"));
    await renameOnRemote("repo", "main", "dev", "origin");
    expect(commands.opDeleteRemoteBranch).not.toHaveBeenCalled();
    expect(notices[0]!.message).toMatch(/^Renamed branch main to dev\. Pushed it to origin\/dev, but it still tracks origin\/main, so main was not deleted on origin\./);
    expect(notices[0]!.action?.label).toBe("Track Remote Branch…");
  });

  it("says to change the default branch on the server when the remote will not delete it", async () => {
    commands.opDeleteRemoteBranch.mockReturnValueOnce(failed(" ! [remote rejected] main (refusing to delete the current branch: refs/heads/main)"));
    await renameOnRemote("repo", "main", "dev", "origin");
    expect(notices[0]!.message).toBe(
      "Renamed branch main to dev. Pushed it to origin/dev, and it now tracks that, but origin/main is still there. " +
        "origin will not delete main because it is the remote's default branch. Make dev the default branch in the repository's settings on the server, then delete origin/main.",
    );
    expect(notices[0]!.action?.label).toBe("Delete Remote Branch…");
  });
});

describe("a sync whose push goes somewhere other than the branch it pulls from", () => {
  it("says the push target has to be pulled, instead of offering to sync again", () => {
    const result: SyncResult = {
      target: { remote: "origin", branch: "shared" },
      pushTarget: { remote: "origin", branch: "my-work" },
      pull: { ok: true, exitCode: 0, stdout: "Already up to date.", stderr: "", conflicts: false },
      push: { ok: false, exitCode: 1, stdout: "", stderr: " ! [rejected]        my-work -> my-work (fetch first)", conflicts: false },
      nothingToPush: false,
      pushStopped: false,
      rewrotePushed: false,
    };
    const outcome = syncOutcome("repo", result, "my-work", "merge");
    expect(outcome.message).toBe(
      "origin/my-work has commits that my-work does not have, so the push was refused. Sync only pulls origin/shared, so pull origin/my-work into my-work first, then push again.",
    );
    expect(outcome.action?.label).toBe("Pull…");
  });
});

describe("failure messages lead with Git's reason", () => {
  const stderr = [
    "Enumerating objects: 7, done.",
    "Counting objects: 100% (7/7), done.",
    "Writing objects: 100% (4/4), 380 bytes | 380.00 KiB/s, done.",
    "Total 4 (delta 1), reused 0 (delta 0), pack-reused 0",
    "remote: main is a protected branch on this server. Push to another branch instead.",
    "To stashpanda-git:git-testbeds/protected-main.git",
    " ! [remote rejected] main -> main (pre-receive hook declined)",
    "error: failed to push some refs to 'stashpanda-git:git-testbeds/protected-main.git'",
  ].join("\n");

  it("drops the progress lines", () => {
    expect(gitReason(stderr)).toBe(
      [
        "remote: main is a protected branch on this server. Push to another branch instead.",
        "! [remote rejected] main -> main (pre-receive hook declined)",
        "error: failed to push some refs to 'stashpanda-git:git-testbeds/protected-main.git'",
      ].join("\n"),
    );
    expect(opFailureText({ ok: false, exitCode: 1, stdout: "", stderr, conflicts: false }, "Pushing main to origin/main")).toMatch(
      /^Pushing main to origin\/main failed \(exit 1\): remote: main is a protected branch/,
    );
  });

  it("keeps everything when there is nothing but progress", () => {
    expect(gitReason("Counting objects: 3, done.\r\nTotal 3 (delta 0)")).toBe("Counting objects: 3, done.\nTotal 3 (delta 0)");
  });
});

describe("a rebase that finished but could not move a branch rebase.updateRefs listed", () => {
  const stderr =
    "Successfully rebased and updated refs/heads/main.\nerror: update_ref failed for ref 'refs/heads/topic': cannot lock ref 'refs/heads/topic': is at c3fab5f but expected a729c2a\nFailed to update the following refs with --update-refs:\n\trefs/heads/topic\n";

  it("names the branches left behind", () => {
    expect(updateRefsLeftBehind(stderr)).toEqual(["topic"]);
    expect(updateRefsLeftBehind("error: could not apply abc123")).toBeNull();
  });

  it("reports the pull as done, with a note, not as a failure", () => {
    const outcome = pullOutcome({ ok: false, exitCode: 1, stdout: "", stderr, conflicts: false }, "origin/main", "main", "rebase");
    expect(outcome.failed).toBe(false);
    expect(outcome.message).toMatch(/^Pulled origin\/main into main, with main's own commits rebased on top\. But topic was not moved along with it/);
  });
});

describe("a second click on Sync while the first is getting ready", () => {
  it("is refused, so only one sync runs", async () => {
    session();
    commands.getPushTarget.mockReturnValue(ok({ remote: "origin", branch: "main" }));
    commands.opSync.mockReturnValue(
      ok({
        target: { remote: "origin", branch: "main" },
        pushTarget: { remote: "origin", branch: "main" },
        pull: { ok: true, exitCode: 0, stdout: "Already up to date.", stderr: "", conflicts: false },
        push: null,
        nothingToPush: true,
        pushStopped: false,
        rewrotePushed: false,
      }),
    );
    const first = syncCurrentBranch(sessionMap.get("repo") as never);
    await syncCurrentBranch(sessionMap.get("repo") as never);
    await first;
    expect(commands.opSync).toHaveBeenCalledTimes(1);
    expect(notices[0]!.message).toBe("Wait for the current operation to finish.");
  });
});

describe("fast-forwarding a local branch to its upstream", () => {
  const local = { kind: { type: "localBranch" } as const, upstream: "refs/remotes/origin/main", upstreamGone: false };

  it("is offered only for a branch that is behind its upstream and has nothing of its own", () => {
    expect(upstreamFastForward({ ...local, ahead: 0, behind: 2 } as never)).toBe("origin/main");
    expect(upstreamFastForward({ ...local, ahead: 1, behind: 2 } as never)).toBeNull();
    expect(upstreamFastForward({ ...local, ahead: 0, behind: 0 } as never)).toBeNull();
    expect(upstreamFastForward({ ...local, upstreamGone: true, ahead: 0, behind: 2 } as never)).toBeNull();
    expect(upstreamFastForward({ ...local, upstream: null, ahead: 0, behind: 2 } as never)).toBeNull();
    expect(upstreamFastForward({ ...local, kind: { type: "remoteBranch", remote: "origin" }, ahead: 0, behind: 2 } as never)).toBeNull();
  });

  it("asks before stashing uncommitted changes around a fast-forward of the checked-out branch", async () => {
    const s = session() as unknown as { status: { unstagedCount: number } };
    s.status.unstagedCount = 1;
    dialogs.open.mockResolvedValue(true);
    commands.opBranchFastForwardToUpstream.mockReturnValue(ok(pushed));
    await fastForwardToUpstream("repo", "feature", "origin/feature");
    expect(dialogs.open).toHaveBeenCalledWith("confirm", expect.objectContaining({ title: "Fast-forward with uncommitted changes?" }));
    expect(commands.opBranchFastForwardToUpstream).toHaveBeenCalledWith("repo", "feature", true);
    expect(notices[notices.length - 1]!.message).toBe("Fast-forwarded feature to origin/feature.");
  });

  it("does nothing when that question is cancelled", async () => {
    const s = session() as unknown as { status: { unstagedCount: number } };
    s.status.unstagedCount = 1;
    dialogs.open.mockResolvedValue(false);
    await fastForwardToUpstream("repo", "feature", "origin/feature");
    expect(commands.opBranchFastForwardToUpstream).not.toHaveBeenCalled();
  });

  it("does not ask for a branch that is not checked out, whose move leaves the files alone", async () => {
    const s = session() as unknown as { status: { unstagedCount: number } };
    s.status.unstagedCount = 1;
    commands.opBranchFastForwardToUpstream.mockReturnValue(ok(pushed));
    await fastForwardToUpstream("repo", "release", "origin/release");
    expect(dialogs.open).not.toHaveBeenCalled();
    expect(commands.opBranchFastForwardToUpstream).toHaveBeenCalledWith("repo", "release", false);
  });
});

describe("merging with uncommitted changes", () => {
  /** feature checked out with an unstaged change, and a second local branch to merge. */
  function dirty() {
    const s = session() as unknown as { status: { unstagedCount: number }; refs: { refs: { kind: { type: string }; short: string; fullName?: string; upstream: string | null }[] } };
    s.status.unstagedCount = 1;
    s.refs.refs.push({ kind: { type: "localBranch" }, short: "topic", fullName: "refs/heads/topic", upstream: null });
    return s;
  }

  it("no longer blocks the Merge or Rebase dialog", () => {
    const s = dirty();
    expect(mergeRebaseReason(s as never, "merge")).toBeNull();
    expect(mergeRebaseReason(s as never, "rebase")).toBeNull();
  });

  it("asks, then merges with the changes stashed around it", async () => {
    dirty();
    dialogs.open.mockResolvedValue(true);
    commands.opMerge.mockReturnValue(ok(pushed));
    await mergeInto("repo", "topic");
    expect(dialogs.open).toHaveBeenCalledWith("confirm", expect.objectContaining({ title: "Merge with uncommitted changes?" }));
    expect(commands.opMerge).toHaveBeenCalledWith("repo", "topic", false, "feature", true);
  });

  it("merges nothing when that question is cancelled", async () => {
    dirty();
    dialogs.open.mockResolvedValue(false);
    await mergeInto("repo", "topic");
    expect(commands.opMerge).not.toHaveBeenCalled();
  });

  it("says where the changes are when the merge stops on conflicts", () => {
    const stopped = { ok: false, exitCode: 1, stdout: "CONFLICT (content): Merge conflict in c.txt", stderr: "", conflicts: true };
    expect(mergeOutcome(stopped, "topic", "feature", true).message).toMatch(/Your uncommitted changes are set aside, and come back when the merge is finished or aborted\.$/);
    expect(mergeOutcome(stopped, "topic", "feature", false).message).not.toMatch(/set aside/);
  });
});

describe("a remote that refused the SSH key", () => {
  const denied = (stderr: string) => ({ ok: false, exitCode: 128, stdout: "", stderr, conflicts: false });
  const refusedLine = "git@github.com: Permission denied (publickey).\nfatal: Could not read from remote repository.";

  it("finds the login in SSH's refusal, and nothing in other failures", () => {
    expect(sshDeniedDestination(refusedLine)).toBe("git@github.com");
    expect(sshDeniedDestination("me@git.example.com: Permission denied (publickey,password).")).toBe("me@git.example.com");
    expect(sshDeniedDestination("fatal: could not read Username for 'https://github.com'")).toBeNull();
    expect(sshDeniedDestination("error: cannot open .git/FETCH_HEAD: Permission denied")).toBeNull();
  });

  it("offers Diagnose…, which opens the dialog with the port from the remote's URL", () => {
    sessionMap.set("k", { refs: { remotes: [{ name: "origin", url: "ssh://git@git.example.com:2222/team/app.git", pushUrl: null }] } });
    const action = sshDiagnoseAction("k", denied("git@git.example.com: Permission denied (publickey)."));
    expect(action?.label).toBe("Diagnose…");
    action?.run();
    expect(dialogs.open).toHaveBeenCalledWith("sshDiagnose", { key: "k", dest: "git@git.example.com", port: 2222 });
    sshDiagnoseAction("k", denied(refusedLine))?.run();
    expect(dialogs.open).toHaveBeenLastCalledWith("sshDiagnose", { key: "k", dest: "git@github.com", port: null });
    expect(sshDiagnoseAction("k", denied("! [rejected] main -> main (fetch first)"))).toBeNull();
    sessionMap.delete("k");
  });

  it("puts Diagnose… on the notice of a push the remote refused", async () => {
    commands.opPush.mockReturnValue(ok(denied(refusedLine)));
    await pushBranch(session(), "main");
    expect(notices[notices.length - 1]?.action?.label).toBe("Diagnose…");
  });
});

describe("the Push button once a push has finished", () => {
  it("is enabled again straight away, without waiting for something else to redraw it", async () => {
    // Reactive like the store's sessions, so the button follows `operation` as it does in the app.
    const s = reactive(session() as unknown as { operation: unknown; key: string });
    sessionMap.set("repo", s);
    // Recomputed the moment anything it read changes, as the toolbar redraws while the push is
    // still finishing.
    const seen: (string | null)[] = [];
    watchEffect(() => seen.push(busyReason(s as never)), { flush: "sync" });
    await pushCurrentBranch(s as never);
    expect(seen.some((r) => r?.startsWith("Wait for the current operation to finish ("))).toBe(true);
    expect(seen[seen.length - 1]).toBeNull();
  });
});
