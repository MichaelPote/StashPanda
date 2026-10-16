import { beforeEach, describe, expect, it, vi } from "vitest";
import type { PushTarget, SyncResult } from "@/lib/bindings";

const commands = vi.hoisted(() => ({
  getPushTarget: vi.fn(),
  getRewrotePushedCommits: vi.fn(),
  getUpstream: vi.fn(),
  getPullMode: vi.fn(),
  opPush: vi.fn(),
  opPushTo: vi.fn(),
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

import { gitReason, opFailureText, pushBranch, pushBranchTo, pushCurrentBranch, syncCurrentBranch, syncOutcome } from "./repoActions";

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

  it("starts with Force push ticked when a notice asked for it", async () => {
    withBranches();
    dialogs.open.mockResolvedValue(null);
    await pushBranchTo("repo", "topic", { remote: "origin", target: "release", force: true });
    expect(dialogs.open).toHaveBeenCalledWith("pushTo", expect.objectContaining({ server: 1, target: "release", force: true }));
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

