/**
 * Git write actions shared by the commit context menu, the branch context menu, the Branches
 * panel and the native menu bar. Every function reports through `sessions.notify`, refetches
 * refs and status afterwards, and never throws.
 */
import { commands, type OpResult } from "@/lib/bindings";
import { errorMessage, unwrap, type IpcResult } from "@/lib/ipc";
import { shortSha } from "@/lib/refNames";
import { useDialogsStore } from "@/stores/dialogs";
import { useSessionsStore, type RepoSession } from "@/stores/sessions";

/** Runs one write operation and refetches refs and status once it has finished, whatever the outcome. */
export async function runOp(key: string, op: Promise<IpcResult<OpResult>>): Promise<OpResult> {
  const sessions = useSessionsStore();
  try {
    return await unwrap(op);
  } finally {
    void sessions.refetchRefsAndStatus(key);
  }
}

/** One line for a Git command that ran but exited non-zero: `<what> failed (exit 1): <stderr>`. */
export function opFailureText(result: OpResult, what: string): string {
  const code = result.exitCode === null ? "" : ` (exit ${result.exitCode})`;
  const detail = result.stderr.trim() || result.stdout.trim();
  return detail.length > 0 ? `${what} failed${code}: ${detail}` : `${what} failed${code}.`;
}

/**
 * Why checkout and merge are blocked right now, or null when the working tree is clean.
 * Untracked files do not block either operation, so they are not counted.
 */
export function dirtyTreeReason(session: RepoSession | null | undefined): string | null {
  const status = session?.status;
  if (!status) return null;
  const parts: string[] = [];
  if (status.conflictedCount > 0) parts.push(`${status.conflictedCount} conflicted`);
  if (status.stagedCount > 0) parts.push(`${status.stagedCount} staged`);
  if (status.unstagedCount > 0) parts.push(`${status.unstagedCount} unstaged`);
  if (parts.length === 0) return null;
  return `The working tree has uncommitted changes (${parts.join(", ")}). Commit or stash them first.`;
}

/** Name of the checked-out branch, or null when HEAD is detached or status is not loaded yet. */
export function currentBranch(session: RepoSession | null | undefined): string | null {
  return session?.status?.branch ?? null;
}

/** True when status says HEAD is detached (falls back to refs, then to "not detached"). */
export function isDetached(session: RepoSession | null | undefined): boolean {
  return session?.status?.detached ?? session?.refs?.detached ?? false;
}

/** Runs `git checkout <target>`; `label` is how the notice names it (a branch or a short sha). */
export async function checkout(key: string, target: string, label: string = target): Promise<void> {
  const sessions = useSessionsStore();
  try {
    const result = await runOp(key, commands.opCheckout(key, target));
    if (result.ok) sessions.notify(`Checked out ${label}.`);
    else sessions.notify(opFailureText(result, `Checking out ${label}`));
  } catch (e) {
    sessions.notify(errorMessage(e));
  }
}

/** Runs `git merge --abort` and reports the outcome. */
export async function abortMerge(key: string): Promise<void> {
  const sessions = useSessionsStore();
  try {
    const result = await runOp(key, commands.opMergeAbort(key));
    if (result.ok) sessions.notify("The merge was aborted.");
    else sessions.notify(opFailureText(result, "Aborting the merge"));
  } catch (e) {
    sessions.notify(errorMessage(e));
  }
}

/**
 * Merges `target` (a branch name or an oid) into the checked-out branch. A conflicted merge
 * leaves the notice with an "Abort merge" button.
 */
export async function mergeInto(key: string, target: string, label: string = target): Promise<void> {
  const sessions = useSessionsStore();
  const into = currentBranch(sessions.sessions.get(key)) ?? "the current branch";
  try {
    const result = await runOp(key, commands.opMerge(key, target, false));
    if (result.ok) {
      sessions.notify(`Merged ${label} into ${into}.`);
    } else if (result.conflicts) {
      sessions.notify("Merge stopped on conflicts. Resolve them or abort.", {
        label: "Abort merge",
        run: () => abortMerge(key),
      });
    } else {
      sessions.notify(opFailureText(result, `Merging ${label} into ${into}`));
    }
  } catch (e) {
    sessions.notify(errorMessage(e));
  }
}

/** Opens the Create Branch dialog starting at `startPoint`. The dialog runs the command itself. */
export async function createBranchHere(key: string, startPoint: string, startLabel: string): Promise<void> {
  const dialogs = useDialogsStore();
  await dialogs.open("createBranch", { key, startPoint, startLabel });
}

/** Opens the Create Tag dialog for `target`. The dialog runs the command itself. */
export async function createTagAt(key: string, target: string, targetLabel: string): Promise<void> {
  const dialogs = useDialogsStore();
  await dialogs.open("createTag", { key, target, targetLabel });
}

/** Asks for confirmation, then runs `git tag -d <name>`. */
export async function removeTag(key: string, name: string): Promise<void> {
  const dialogs = useDialogsStore();
  const sessions = useSessionsStore();
  const confirmed = await dialogs.open("confirm", {
    title: `Remove tag ${name}?`,
    message: `The tag ${name} is deleted from this repository. Copies on remotes are not touched.`,
    confirmLabel: "Remove tag",
    destructive: true,
  });
  if (!confirmed) return;
  try {
    const result = await runOp(key, commands.opTagDelete(key, name));
    if (result.ok) sessions.notify(`Removed tag ${name}.`);
    else sessions.notify(opFailureText(result, `Removing tag ${name}`));
  } catch (e) {
    sessions.notify(errorMessage(e));
  }
}

/**
 * Asks for confirmation, then runs `git branch -d <name>`. When Git refuses because the branch
 * is not fully merged, offers `git branch -D <name>` in a second confirmation.
 */
export async function deleteBranch(key: string, name: string): Promise<void> {
  const dialogs = useDialogsStore();
  const sessions = useSessionsStore();
  const confirmed = await dialogs.open("confirm", {
    title: `Delete branch ${name}?`,
    message: `The local branch ${name} is deleted. Git refuses if it has commits that are not merged yet; you can force it then.`,
    confirmLabel: "Delete branch",
    destructive: true,
  });
  if (!confirmed) return;
  try {
    let result = await runOp(key, commands.opBranchDelete(key, name, false));
    if (!result.ok && /not fully merged/i.test(result.stderr)) {
      const force = await dialogs.open("confirm", {
        title: `Force delete ${name}?`,
        message: `${result.stderr.trim()}\n\nDeleting it anyway makes those commits unreachable.`,
        confirmLabel: "Force delete",
        destructive: true,
      });
      if (!force) return;
      result = await runOp(key, commands.opBranchDelete(key, name, true));
    }
    if (result.ok) sessions.notify(`Deleted branch ${name}.`);
    else sessions.notify(opFailureText(result, `Deleting branch ${name}`));
  } catch (e) {
    sessions.notify(errorMessage(e));
  }
}

/** Writes `text` to the clipboard; `what` names it in the failure notice ("the SHA", "the message"). */
export async function copyText(text: string, what: string): Promise<void> {
  const sessions = useSessionsStore();
  try {
    await navigator.clipboard.writeText(text);
  } catch (e) {
    sessions.notify(`Could not copy ${what} to the clipboard: ${String(e)}`);
  }
}

/** Fetches the full commit message and copies it. */
export async function copyCommitMessage(key: string, oid: string): Promise<void> {
  const sessions = useSessionsStore();
  try {
    const message = await unwrap(commands.getCommitMessage(key, oid));
    await copyText(message, `the message of ${shortSha(oid)}`);
  } catch (e) {
    sessions.notify(errorMessage(e));
  }
}

/** For a remote branch such as `origin/dev`, the part after the remote name: `dev`. */
export function stripRemote(short: string, remote: string): string {
  const prefix = `${remote}/`;
  return short.startsWith(prefix) ? short.slice(prefix.length) : short;
}
