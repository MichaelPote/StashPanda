import { beforeEach, describe, expect, it, vi } from "vitest";
import type { InProgress, OpResult, RefEntry, RefKind } from "@/lib/bindings";
import type { RepoSession } from "@/stores/sessions";

const mocks = vi.hoisted(() => ({
  opBranchCreate: vi.fn(),
  opBranchSetUpstream: vi.fn(),
  open: vi.fn(),
  sessions: new Map<string, unknown>(),
}));

vi.mock("@/lib/bindings", () => ({ commands: { opBranchCreate: mocks.opBranchCreate, opBranchSetUpstream: mocks.opBranchSetUpstream } }));
vi.mock("@tauri-apps/plugin-opener", () => ({ openPath: vi.fn(), revealItemInDir: vi.fn() }));
vi.mock("@/stores/dialogs", () => ({ useDialogsStore: () => ({ open: mocks.open }) }));
vi.mock("@/stores/library", () => ({ repoDisplayName: vi.fn(), useLibraryStore: vi.fn() }));
vi.mock("@/stores/settings", () => ({ useSettingsStore: vi.fn() }));
vi.mock("@/stores/uiState", () => ({ useUiStateStore: vi.fn() }));
vi.mock("@/stores/sessions", () => ({
  useSessionsStore: () => ({ sessions: mocks.sessions, notify: vi.fn(), notifySuccess: vi.fn(), refetchRefsAndStatus: vi.fn() }),
}));

const { checkoutTargetOf, continueOutcome, openMergeDialog, rebaseOutcome, requestCheckout, resetReason, revisionOf, tagPushOutcome, trackRemoteBranch } =
  await import("./repoActions");

const ref = (fullName: string, kind: RefKind): RefEntry =>
  ({ fullName, short: fullName.replace(/^refs\/(heads|remotes|tags)\//, ""), kind, target: "c1" }) as RefEntry;
const session = (refs: RefEntry[], inProgress: InProgress | null = null) =>
  ({
    key: "k",
    info: { isBare: false },
    refs: { refs },
    status: { branch: "main", headOid: "c0", detached: false, inProgress, entries: [], conflictedCount: 0 },
  }) as unknown as RepoSession;

beforeEach(() => {
  mocks.opBranchCreate.mockReset().mockResolvedValue({ status: "ok", data: { ok: true, stdout: "", stderr: "" } });
  mocks.opBranchSetUpstream.mockReset().mockResolvedValue({ status: "ok", data: { ok: true, stdout: "", stderr: "" } });
  mocks.open.mockReset().mockResolvedValue(true);
  mocks.sessions.clear();
});

describe("Track Remote Branch", () => {
  it("offers the remote branch of the same name, on origin first, when the branch tracks nothing", async () => {
    const local = { ...ref("refs/heads/feature/x", { type: "localBranch" }), upstream: null };
    const other = ref("refs/remotes/fork/feature/x", { type: "remoteBranch", remote: "fork" });
    const origin = ref("refs/remotes/origin/feature/x", { type: "remoteBranch", remote: "origin" });
    mocks.sessions.set("k", session([local, other, origin, ref("refs/remotes/origin/main", { type: "remoteBranch", remote: "origin" })]));
    await trackRemoteBranch("k", "feature/x");
    expect(mocks.open).toHaveBeenCalledWith("trackRemoteBranch", expect.objectContaining({ upstream: null, suggested: "refs/remotes/origin/feature/x" }));
  });

  it("suggests nothing when the branch already tracks one", async () => {
    const local = { ...ref("refs/heads/feature/x", { type: "localBranch" }), upstream: "refs/remotes/origin/main" };
    mocks.sessions.set("k", session([local, ref("refs/remotes/origin/feature/x", { type: "remoteBranch", remote: "origin" })]));
    await trackRemoteBranch("k", "feature/x");
    expect(mocks.open).toHaveBeenCalledWith("trackRemoteBranch", expect.objectContaining({ upstream: "refs/remotes/origin/main", suggested: null }));
  });
});

describe("refs given to Git", () => {
  it("checks out a tag by its full name, so a branch of the same name is not picked", () => {
    expect(checkoutTargetOf(ref("refs/tags/v1", { type: "tag", annotated: false }))).toEqual({ kind: "revision", revision: "refs/tags/v1", label: "v1" });
    expect(checkoutTargetOf(ref("refs/heads/v1", { type: "localBranch" }))).toEqual({ kind: "local", name: "v1" });
    expect(checkoutTargetOf(ref("refs/remotes/team/origin/dev", { type: "remoteBranch", remote: "team/origin" }))).toEqual({
      kind: "remote",
      remote: "team/origin/dev",
      local: "dev",
    });
  });

  it("starts a branch for a remote branch at its full name, then makes it track that as its own step", async () => {
    mocks.sessions.set("k", session([ref("refs/remotes/origin/dev", { type: "remoteBranch", remote: "origin" })]));
    await requestCheckout("k", { kind: "remote", remote: "origin/dev", local: "dev" });
    expect(mocks.opBranchCreate).toHaveBeenCalledWith("k", "dev", "refs/remotes/origin/dev", true);
    expect(mocks.opBranchSetUpstream).toHaveBeenCalledWith("k", "dev", "refs/remotes/origin/dev");
  });

  it("merges by the short name unless another ref has it too", async () => {
    const topic = ref("refs/heads/topic", { type: "localBranch" });
    const dev = ref("refs/heads/dev", { type: "localBranch" });
    const refs = [topic, dev, ref("refs/tags/topic", { type: "tag", annotated: false })];
    expect(revisionOf(dev, refs)).toBe("dev");
    expect(revisionOf(topic, refs)).toBe("refs/heads/topic");
    // The Merge dialog is given the same names to pass on.
    await openMergeDialog(session(refs));
    const branches = mocks.open.mock.calls[0]?.[1]?.branches as { short: string; revision: string }[];
    expect(branches.map((b) => [b.short, b.revision])).toEqual([
      ["topic", "refs/heads/topic"],
      ["dev", "dev"],
    ]);
  });
});

describe("reset", () => {
  it("is blocked while a merge or rebase is open", () => {
    expect(resetReason(session([]))).toBeNull();
    const merge = { kind: "merge", commit: "c2", subject: null, branch: null, onto: null, step: null, total: null } as InProgress;
    expect(resetReason(session([], merge))).toMatch(/merge is in progress/);
  });
});

const names = { title: "Rebasing main onto up", ours: "up", theirs: "79fe64e from main" };
const rebasing: InProgress = { kind: "rebase", commit: null, subject: "M", branch: "refs/heads/main", onto: null, step: 1, total: 1, interrupted: false };
const result = (ok: boolean, stdout: string, stderr = "", conflicts = false): OpResult => ({ ok, exitCode: ok ? 0 : 1, stdout, stderr, conflicts });

describe("continuing an operation", () => {
  it("reports a plain finish as a success", () => {
    const done = continueOutcome(result(true, "[detached HEAD aaaef85] M\n 1 file changed"), rebasing, names);
    expect(done).toEqual({ message: "Finished rebasing onto up.", kind: "success" });
  });

  it("says when putting the autostashed changes back conflicted, although Git exits 0", () => {
    // `git rebase --continue` after a pull with --autostash, Git 2.43.
    const stdout =
      "[detached HEAD aaaef85] M\n 1 file changed, 1 insertion(+), 1 deletion(-)\nApplying autostash resulted in conflicts.\n" +
      'Your changes are safe in the stash.\nYou can run "git stash pop" or "git stash drop" at any time.';
    const done = continueOutcome(result(true, stdout, "Successfully rebased and updated refs/heads/main."), rebasing, names);
    expect(done.kind).toBe("error");
    expect(done.message).toBe(
      "Finished rebasing onto up, but putting your uncommitted changes back caused conflicts. Resolve them in the working tree; your changes are also kept as stash@{0}.",
    );
  });

  it("recognises the wording newer Git versions use for the same conflict", () => {
    // As printed after a fast-forward with --autostash by a Git newer than 2.47.
    const stderr =
      "Your local changes are stashed, however applying them\nresulted in conflicts.  You can either resolve the conflicts\n" +
      'and then discard the stash with "git stash drop", or, if you\ndo not want to resolve them now, run "git reset --hard" and\n' +
      'apply the local changes later by running "git stash pop".';
    const done = continueOutcome(result(true, "[detached HEAD aaaef85] M\n 1 file changed", stderr), rebasing, names);
    expect(done.kind).toBe("error");
  });
});

describe("a rebase that stops", () => {
  // `git rebase main` with an untracked x in the way of a replayed commit (Git 2.43): exit 1, no
  // CONFLICT line, and the rebase stays open.
  const blocked = result(false, "", "error: The following untracked working tree files would be overwritten by merge:\n\tx\nPlease move or remove them before you merge.\nAborting");

  it("is a plain failure when nothing is left open", () => {
    expect(rebaseOutcome(blocked, "feature", "main").failed).toBe(true);
  });

  it("is not a plain failure when the rebase is still open", () => {
    const open = rebaseOutcome(blocked, "feature", "main", true);
    expect(open.failed).toBe(false);
    expect(open.kind).toBe("error");
    expect(open.message).toContain("is still open");
    expect(open.message).toContain("would be overwritten by merge");
  });
});

describe("the outcome of pushing tags", () => {
  // Git 2.43's own lines, from `git push --progress origin refs/tags/clash refs/tags/new1`.
  const pushed = (stderr: string, ok = true): OpResult => ({ ok, exitCode: ok ? 0 : 1, stdout: "", stderr, conflicts: false });

  it("names the tags that were pushed", () => {
    const outcome = tagPushOutcome(pushed("To o.git\n * [new tag]         v1 -> v1\n * [new tag]         v2 -> v2"), "origin", "every tag");
    expect(outcome).toEqual({ message: "Pushed v1 and v2 to origin.", failed: false });
  });

  it("says when the remote had them all already", () => {
    expect(tagPushOutcome(pushed("Everything up-to-date"), "origin", "v1")).toEqual({ message: "origin already has v1.", failed: false });
  });

  it("says when there was nothing to push", () => {
    const none: OpResult = { ok: true, exitCode: 0, stdout: "No tags to push.", stderr: "", conflicts: false };
    expect(tagPushOutcome(none, "origin", "the tags on main")).toEqual({ message: "There were no tags to push to origin.", failed: false });
  });

  it("explains a tag the remote has on another commit, and still names those that went", () => {
    const stderr =
      "To o.git\n * [new tag]         new1 -> new1\n ! [rejected]        clash -> clash (already exists)\n" +
      "error: failed to push some refs to 'o.git'\nhint: Updates were rejected because the tag already exists in the remote.";
    const outcome = tagPushOutcome(pushed(stderr, false), "origin", "every tag");
    expect(outcome.failed).toBe(true);
    expect(outcome.message).toBe(
      "origin already has a tag clash on another commit, so it was not pushed: a push never moves a tag on a remote, since others may already be using it. Move Tag… in the tag's menu can replace it there. Pushed new1.",
    );
  });

  it("lists many tags briefly", () => {
    const lines = ["a", "b", "c", "d", "e"].map((t) => ` * [new tag]         ${t} -> ${t}`).join("\n");
    expect(tagPushOutcome(pushed(lines), "origin", "every tag").message).toBe("Pushed a, b, c and 2 more to origin.");
  });
});
