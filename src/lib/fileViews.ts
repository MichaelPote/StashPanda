/**
 * Opening a file's history and blame, from the Files panel's right-click menu and the diff
 * panel's header, and going from those dialogs back to the commit graph.
 */
import type { DiffTarget } from "@/lib/bindings";
import { shortOid } from "@/lib/format";
import { useDialogsStore } from "@/stores/dialogs";
import { useSessionsStore } from "@/stores/sessions";

/**
 * The version of a file that its history and blame start from, for a diff the app shows: a
 * commit's diff starts at that commit; a working tree diff starts at HEAD for the history and
 * at the working tree for the blame (null in both cases).
 */
export function fileViewRev(target: DiffTarget): string | null {
  return target.type === "commit" ? target.commit : null;
}

/** Why a file's history and blame cannot be shown, or null when they can. */
export function fileViewReason(target: DiffTarget | null): string | null {
  if (!target) return "Select a file first.";
  if (target.type === "untracked") return "Git does not track this file yet, so it has no history and no commits to blame.";
  return null;
}

/** Opens the File History dialog for `path` as of `rev` (null for HEAD). */
export function openFileHistory(key: string, path: string, rev: string | null): void {
  void useDialogsStore().open("fileHistory", { key, path, rev });
}

/** Opens the Blame dialog for `path` as it is in `rev` (null for the working tree), scrolled to `line` when given. */
export function openBlame(key: string, path: string, rev: string | null, line?: number): void {
  void useDialogsStore().open("blame", { key, path, rev, line });
}

/** Conflict letters of a file both sides still have, which the merge editor can open. */
const MERGEABLE = new Set(["UU", "AA"]);

/** Why the merge editor cannot open a conflicted file with these status letters, or null. */
export function mergeEditorReason(letters: string): string | null {
  if (MERGEABLE.has(letters)) return null;
  return "One side deleted this file, so there is no text to merge: take one side's version instead.";
}

/** Opens the three-pane merge editor on conflicted file `path` of repository `key`. */
export function openMergeEditor(key: string, path: string): void {
  void useDialogsStore().open("mergeEditor", { key, path });
}

/**
 * Closes the open dialog, then selects `oid` in the commit graph of repository `key` and scrolls
 * to it. Says so when the graph does not show that commit.
 */
export async function showCommitInGraph(key: string, oid: string): Promise<void> {
  const dialogs = useDialogsStore();
  const sessions = useSessionsStore();
  dialogs.cancel();
  if (sessions.activeKey !== key) sessions.setActive(key);
  const found = await sessions.selectCommit(key, oid);
  if (!found) {
    sessions.notify(`Commit ${shortOid(oid)} is not in the commit graph. It may be on a branch that is hidden, or on no branch at all.`);
  }
}
