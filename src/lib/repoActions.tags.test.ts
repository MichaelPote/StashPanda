import { beforeEach, describe, expect, it, vi } from "vitest";

const commands = vi.hoisted(() => ({
  opTagDelete: vi.fn(),
  opDeleteRemoteTag: vi.fn(),
  opPushTags: vi.fn(),
  opPushTagForce: vi.fn(),
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

import { moveTag, removeTag, renameTag } from "./repoActions";

const ok = <T>(data: T) => Promise.resolve({ status: "ok", data });
const done = { ok: true, exitCode: 0, stdout: "", stderr: "", conflicts: false };
const failed = (stderr: string) => ok({ ok: false, exitCode: 1, stdout: "", stderr, conflicts: false });
const remoteTag = { oid: "a1a1a1a1", commit: "c0ffee00" };

beforeEach(() => {
  vi.clearAllMocks();
  notices.length = 0;
  sessionMap.set("repo", { key: "repo", operation: null, refs: { remotes: [{ name: "origin" }, { name: "backup" }], refs: [] } });
  commands.opTagDelete.mockReturnValue(ok(done));
  commands.opDeleteRemoteTag.mockReturnValue(ok(done));
  commands.opPushTags.mockReturnValue(ok(done));
  commands.opPushTagForce.mockReturnValue(ok(done));
});

describe("removing a tag", () => {
  it("removes it from the ticked remotes first, then here", async () => {
    dialogs.open.mockResolvedValue({ remotes: [{ remote: "origin", found: remoteTag }, { remote: "backup", found: undefined }] });
    await removeTag("repo", "v1");
    expect(dialogs.open).toHaveBeenCalledWith("removeTag", { key: "repo", name: "v1" });
    expect(commands.opDeleteRemoteTag.mock.calls.map((c) => c[2])).toEqual(["origin", "backup"]);
    expect(commands.opDeleteRemoteTag.mock.invocationCallOrder[1]!).toBeLessThan(commands.opTagDelete.mock.invocationCallOrder[0]!);
    expect(notices.map((n) => n.message)).toEqual(["Removed tag v1 here and from origin and backup."]);
  });

  it("counts a remote that no longer has the tag as done", async () => {
    dialogs.open.mockResolvedValue({ remotes: [{ remote: "origin", found: undefined }, { remote: "backup", found: undefined }] });
    commands.opDeleteRemoteTag.mockReturnValueOnce(ok({ ...done, stderr: "remote: warning: deleting a non-existent ref" }));
    commands.opDeleteRemoteTag.mockReturnValueOnce(failed("error: unable to delete 'v1': remote ref does not exist"));
    await removeTag("repo", "v1");
    expect(notices.map((n) => n.message)).toEqual(["Removed tag v1 here and from origin and backup."]);
  });

  it("keeps the tag here when a remote would not remove it, so it can be tried again", async () => {
    dialogs.open.mockResolvedValue({ remotes: [{ remote: "origin", found: remoteTag }, { remote: "backup", found: remoteTag }] });
    commands.opDeleteRemoteTag.mockReturnValueOnce(ok(done)).mockReturnValueOnce(failed(" ! [remote rejected] v1 (pre-receive hook declined)"));
    await removeTag("repo", "v1");
    expect(commands.opTagDelete).not.toHaveBeenCalled();
    expect(notices[0]!.message).toMatch(/^Removing tag v1 from backup failed \(exit 1\): .*The tag is still here; it was removed from origin\.$/);
  });

  it("removes it only here with nothing ticked, and does nothing when cancelled", async () => {
    dialogs.open.mockResolvedValueOnce({ remotes: [] }).mockResolvedValueOnce(null);
    await removeTag("repo", "v1");
    await removeTag("repo", "v1");
    expect(commands.opDeleteRemoteTag).not.toHaveBeenCalled();
    expect(commands.opTagDelete).toHaveBeenCalledTimes(1);
    expect(notices.map((n) => n.message)).toEqual(["Removed tag v1."]);
  });
});

describe("renaming a tag on remotes", () => {
  it("pushes the new name, then deletes the old one, on each remote in turn", async () => {
    dialogs.open.mockResolvedValue({ name: "v1.0", remotes: [{ remote: "origin", found: remoteTag }, { remote: "backup", found: remoteTag }] });
    await renameTag("repo", "v1");
    expect(commands.opPushTags).toHaveBeenNthCalledWith(1, "repo", expect.any(String), "origin", { type: "named", names: ["v1.0"] });
    expect(commands.opDeleteRemoteTag).toHaveBeenNthCalledWith(1, "repo", expect.any(String), "origin", "v1");
    expect(commands.opPushTags.mock.invocationCallOrder[0]!).toBeLessThan(commands.opDeleteRemoteTag.mock.invocationCallOrder[0]!);
    expect(commands.opDeleteRemoteTag.mock.calls.map((c) => c[2])).toEqual(["origin", "backup"]);
    expect(notices.map((n) => n.message)).toEqual(["Renamed tag v1 to v1.0 here and on origin and backup."]);
  });

  it("deletes nothing on a remote that refused the new name", async () => {
    dialogs.open.mockResolvedValue({ name: "v1.0", remotes: [{ remote: "origin", found: remoteTag }] });
    commands.opPushTags.mockReturnValueOnce(failed(" ! [rejected]        v1.0 -> v1.0 (already exists)"));
    await renameTag("repo", "v1");
    expect(commands.opDeleteRemoteTag).not.toHaveBeenCalled();
    expect(notices[0]!.message).toBe("Renamed tag v1 to v1.0. origin already has a tag v1.0 on another commit, so the push was refused. origin still has v1.");
  });

  it("says nothing more when no remote was ticked: the dialog has reported the rename", async () => {
    dialogs.open.mockResolvedValue({ name: "v1.0", remotes: [] });
    await renameTag("repo", "v1");
    expect(commands.opPushTags).not.toHaveBeenCalled();
    expect(notices).toEqual([]);
  });
});

describe("moving a tag on remotes", () => {
  it("leases on what the dialog found: the remote's tag, none, or a plain --force when it had not answered", async () => {
    dialogs.open.mockResolvedValue({
      targetLabel: "beef123",
      remotes: [{ remote: "origin", found: remoteTag }, { remote: "backup", found: null }, { remote: "mirror", found: undefined }],
    });
    await moveTag("repo", "v1");
    expect(commands.opPushTagForce.mock.calls.map((c) => [c[2], c[3], c[4]])).toEqual([
      ["origin", "v1", "a1a1a1a1"],
      ["backup", "v1", ""],
      ["mirror", "v1", null],
    ]);
    expect(notices.map((n) => n.message)).toEqual(["Moved tag v1 to beef123 here and on origin, backup and mirror."]);
  });

  it("says when the remote's tag changed after the check", async () => {
    dialogs.open.mockResolvedValue({ targetLabel: "beef123", remotes: [{ remote: "origin", found: remoteTag }] });
    commands.opPushTagForce.mockReturnValueOnce(failed(" ! [rejected]        v1 -> v1 (stale info)"));
    await moveTag("repo", "v1");
    expect(notices[0]!.message).toBe(
      "Moved tag v1 to beef123. origin's v1 changed after StashPanda checked it, so it was not replaced. Fetch, look at it, then move the tag again.",
    );
  });
});
