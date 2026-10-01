/**
 * Git write actions shared by the commit context menu, the branch context menu, the Branches
 * panel and the native menu bar, plus removing a repository from the library. Every function
 * reports through `sessions.notify`, refetches refs and status afterwards, and never throws.
 */
import { commands, type OpResult, type PullMode, type StashEntry } from "@/lib/bindings";
import { commitFiles } from "@/lib/files";
import { errorMessage, unwrap, type IpcResult } from "@/lib/ipc";
import { shortSha } from "@/lib/refNames";
import { useDialogsStore, type PullSource } from "@/stores/dialogs";
import { repoDisplayName, useLibraryStore } from "@/stores/library";
import { useSessionsStore, type NoticeAction, type RepoSession } from "@/stores/sessions";
import { useSettingsStore } from "@/stores/settings";
import { useUiStateStore } from "@/stores/uiState";

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
 * The working tree's uncommitted changes, such as `2 staged, 1 unstaged`, or null when there are
 * none. Untracked files do not get in the way of a checkout or a merge, so they are not counted.
 */
export function uncommittedSummary(session: RepoSession | null | undefined): string | null {
  const status = session?.status;
  if (!status) return null;
  const parts: string[] = [];
  if (status.conflictedCount > 0) parts.push(`${status.conflictedCount} conflicted`);
  if (status.stagedCount > 0) parts.push(`${status.stagedCount} staged`);
  if (status.unstagedCount > 0) parts.push(`${status.unstagedCount} unstaged`);
  return parts.length === 0 ? null : parts.join(", ");
}

/** Why a merge is blocked right now, or null when the working tree is clean. */
export function dirtyTreeReason(session: RepoSession | null | undefined): string | null {
  const summary = uncommittedSummary(session);
  return summary === null ? null : `The working tree has uncommitted changes (${summary}). Commit or stash them first.`;
}

/** Name of the checked-out branch, or null when HEAD is detached or status is not loaded yet. */
export function currentBranch(session: RepoSession | null | undefined): string | null {
  return session?.status?.branch ?? null;
}

/** True when status says HEAD is detached (falls back to refs, then to "not detached"). */
export function isDetached(session: RepoSession | null | undefined): boolean {
  return session?.status?.detached ?? session?.refs?.detached ?? false;
}

/**
 * What a checkout goes to: a local branch; a remote branch (`origin/stage`), checked out as the
 * local branch of its name (`stage`); or a commit or tag, which detaches HEAD. `label` is how
 * messages name a revision: a short sha or the tag.
 */
export type CheckoutTarget =
  | { kind: "local"; name: string }
  | { kind: "remote"; remote: string; local: string }
  | { kind: "revision"; revision: string; label: string };

/**
 * Every checkout goes through here, whether it starts from a menu or a double-click:
 * 1. a dirty working tree asks first, since the changes come along to the other branch;
 * 2. a remote branch with no local branch of its name asks first, then creates one that tracks it
 *    (`git checkout -b <local> <remote>`); with a local branch of that name it checks that out;
 * 3. otherwise the checkout runs straight away.
 * When both 1 and 2 apply, one dialog says both.
 */
export async function requestCheckout(key: string, target: CheckoutTarget): Promise<void> {
  const sessions = useSessionsStore();
  const dialogs = useDialogsStore();
  const session = sessions.sessions.get(key);
  let plan = target;
  if (plan.kind === "remote") {
    const local = plan.local;
    if ((session?.refs?.refs ?? []).some((r) => r.kind.type === "localBranch" && r.short === local)) plan = { kind: "local", name: local };
  }
  const name = plan.kind === "local" ? plan.name : plan.kind === "remote" ? plan.local : plan.label;
  const uncommitted = uncommittedSummary(session);

  if (uncommitted !== null || plan.kind === "remote") {
    const sentences: string[] = [];
    if (plan.kind === "remote") {
      sentences.push(
        `Are you sure you want to create a local branch ${plan.local} for ${plan.remote} and check it out? It starts at ${plan.remote} and tracks it.`,
      );
    } else {
      sentences.push(`Are you sure you want to check out ${name}?`);
    }
    if (uncommitted !== null) {
      sentences.push(
        `The working tree has uncommitted changes (${uncommitted}). They are carried over to ${name}; if checking out would overwrite any of them, Git stops and nothing changes.`,
      );
    }
    const confirmed = await dialogs.open("confirm", {
      title: plan.kind === "remote" ? `Create branch ${plan.local}?` : `Check out ${name}?`,
      message: sentences.join(" "),
      confirmLabel: plan.kind === "remote" ? "Create and check out" : "Check out",
    });
    if (!confirmed) return;
  }

  try {
    if (plan.kind === "remote") {
      const result = await runOp(key, commands.opBranchCreate(key, plan.local, plan.remote, true));
      if (result.ok) sessions.notifySuccess(`Created branch ${plan.local} tracking ${plan.remote}, and checked it out.`);
      else sessions.notify(opFailureText(result, `Creating branch ${plan.local} from ${plan.remote}`));
      return;
    }
    const revision = plan.kind === "local" ? plan.name : plan.revision;
    const result = await runOp(key, commands.opCheckout(key, revision));
    if (result.ok) sessions.notifySuccess(`Checked out ${name}.`);
    else sessions.notify(opFailureText(result, `Checking out ${name}`));
  } catch (e) {
    sessions.notify(errorMessage(e));
  }
}

/** Runs `git merge --abort` and reports the outcome. */
export async function abortMerge(key: string): Promise<void> {
  const sessions = useSessionsStore();
  try {
    const result = await runOp(key, commands.opMergeAbort(key));
    if (result.ok) sessions.notifySuccess("The merge was aborted.");
    else sessions.notify(opFailureText(result, "Aborting the merge"));
  } catch (e) {
    sessions.notify(errorMessage(e));
  }
}

/** Runs `git rebase --abort` and reports the outcome. */
export async function abortRebase(key: string): Promise<void> {
  const sessions = useSessionsStore();
  try {
    const result = await runOp(key, commands.opRebaseAbort(key));
    if (result.ok) sessions.notifySuccess("The rebase was aborted. The branch is back where it was before the pull.");
    else sessions.notify(opFailureText(result, "Aborting the rebase"));
  } catch (e) {
    sessions.notify(errorMessage(e));
  }
}

/** Why the active repository cannot pull right now, or null when it can. */
export function pullReason(session: RepoSession | null | undefined): string | null {
  if (!session) return "Open a repository first.";
  if (session.info.isBare) return "A bare repository has no checked-out branch to pull into.";
  if (!session.status || !session.refs) return "The repository is still loading.";
  if (isDetached(session) || currentBranch(session) === null) return "HEAD is detached. Check out a branch before pulling.";
  if (session.status.conflictedCount > 0) return "Resolve the conflicted files first: Git cannot pull while files are unmerged.";
  return null;
}

/**
 * Opens the Pull dialog for the checked-out branch, with its upstream picked. When the working tree
 * has uncommitted changes, asks first; going ahead pulls with `--autostash`. The mode starts as
 * the one last used in this repository, else what the repository's Git config asks for, else
 * Merge. The dialog runs the pull itself and reports the outcome.
 */
export async function openPullDialog(session: RepoSession | null): Promise<void> {
  const sessions = useSessionsStore();
  const dialogs = useDialogsStore();
  const reason = pullReason(session);
  const into = currentBranch(session);
  if (reason || !session?.refs || into === null) {
    sessions.notify(reason ?? "The repository is still loading.");
    return;
  }
  const key = session.key;
  const refs = session.refs.refs;
  const sources: PullSource[] = refs.flatMap((r) =>
    r.kind.type === "remoteBranch" ? [{ short: r.short, remote: r.kind.remote, branch: stripRemote(r.short, r.kind.remote) }] : [],
  );
  if (sources.length === 0) {
    sessions.notify("There are no remote branches to pull from. Add a remote, or fetch from it first.");
    return;
  }
  const upstream = refs.find((r) => r.kind.type === "localBranch" && r.short === into)?.upstream ?? null;
  const upstreamSource = upstream === null ? undefined : sources.find((s) => `refs/remotes/${s.short}` === upstream);
  const source = upstreamSource ?? sources.find((s) => s.short === `origin/${into}`) ?? sources[0];

  const uncommitted = uncommittedSummary(session);
  if (uncommitted !== null) {
    const confirmed = await dialogs.open("confirm", {
      title: "Pull with uncommitted changes?",
      message:
        `The working tree has uncommitted changes (${uncommitted}). Pulling anyway stashes them first, pulls, ` +
        "then puts them back (git pull --autostash). If putting them back conflicts, they also stay in a stash, so nothing is lost.",
      confirmLabel: "Pull anyway (autostash)",
    });
    if (!confirmed) return;
  }

  let mode: PullMode | null = useUiStateStore().state?.perRepo?.[key]?.pullMode ?? null;
  if (mode === null) {
    try {
      mode = await unwrap(commands.getPullMode(key, into));
    } catch {
      // Reading the config is only for the starting choice; Merge is a fine one.
    }
  }
  await dialogs.open("pull", {
    key,
    into,
    sources,
    source,
    sourceIsUpstream: upstreamSource !== undefined && source === upstreamSource,
    mode: mode ?? "merge",
    autostash: uncommitted !== null,
  });
}

/** What the notice strip says after a pull ran to the end, and the button it offers. */
export interface PullOutcome {
  message: string;
  kind: "success" | "error";
  action: NoticeAction | null;
  /** A plain failure: the Pull dialog keeps it open so another mode or branch can be tried. */
  failed: boolean;
}

/** Turns a finished pull into the notice to show. `from` is `origin/dev`, `into` the local branch. */
export function pullOutcome(key: string, result: OpResult, from: string, into: string, mode: PullMode): PullOutcome {
  const output = `${result.stdout}\n${result.stderr}`;
  if (result.ok && /Applying autostash resulted in conflicts/i.test(output)) {
    return {
      message: `Pulled ${from} into ${into}, but putting your uncommitted changes back caused conflicts. Resolve them in the working tree; your changes are also kept as stash@{0}.`,
      kind: "error",
      action: null,
      failed: false,
    };
  }
  if (result.ok) {
    const message = /Already up to date|Current branch .* is up to date/i.test(output)
      ? `${into} is already up to date with ${from}.`
      : /Successfully rebased/i.test(output)
        ? `Pulled ${from} into ${into}, with ${into}'s own commits rebased on top.`
        : /Merge made by/i.test(output)
          ? `Pulled ${from} into ${into} with a merge commit.`
          : `Pulled ${from} into ${into}.`;
    return { message, kind: "success", action: null, failed: false };
  }
  if (result.conflicts && mode === "rebase") {
    return {
      message: `The pull stopped on conflicts while rebasing ${into} onto ${from}. Resolve them, or abort the rebase.`,
      kind: "error",
      action: { label: "Abort rebase", run: () => abortRebase(key) },
      failed: false,
    };
  }
  if (result.conflicts) {
    return {
      message: `The pull stopped on conflicts while merging ${from} into ${into}. Resolve them, or abort the merge.`,
      kind: "error",
      action: { label: "Abort merge", run: () => abortMerge(key) },
      failed: false,
    };
  }
  // GIT_TERMINAL_PROMPT=0 stops Git asking for a username and password on a terminal it does not have.
  const message = /terminal prompts disabled|could not read Username/i.test(output)
    ? `Git needs a username and password for this remote, and StashPanda cannot ask for them yet. Set up a credential helper (Git Credential Manager on Windows), or use an SSH remote.`
    : opFailureText(result, `Pulling ${from} into ${into}`);
  return { message, kind: "error", action: null, failed: true };
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
      sessions.notifySuccess(`Merged ${label} into ${into}.`);
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

/**
 * Opens the Commit dialog listing every outstanding file, with the files selected in the working
 * tree list ticked, or when nothing is selected, what the `commit.preselect` setting says. The
 * dialog runs the commit itself.
 */
export async function openCommitDialog(session: RepoSession | null): Promise<void> {
  const sessions = useSessionsStore();
  const dialogs = useDialogsStore();
  if (!session) {
    sessions.notify("Open a repository first.");
    return;
  }
  if (session.info.isBare) {
    sessions.notify("A bare repository has no working tree to commit from.");
    return;
  }
  const preselect = useSettingsStore().settings?.commit?.preselect ?? "all";
  const { files, ticked, fromSelection } = commitFiles(session.status, session.fileSelection.ids, preselect);
  const hasHead = (session.status?.headOid ?? null) !== null;
  await dialogs.open("commit", { key: session.key, files, ticked, fromSelection, branch: currentBranch(session), hasHead });
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
    if (result.ok) sessions.notifySuccess(`Removed tag ${name}.`);
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
    if (result.ok) sessions.notifySuccess(`Deleted branch ${name}.`);
    else sessions.notify(opFailureText(result, `Deleting branch ${name}`));
  } catch (e) {
    sessions.notify(errorMessage(e));
  }
}

/**
 * Asks for confirmation, then closes the repository's tab and removes it from the library. The
 * files on disk are not touched. Resolves true once it has been removed.
 */
export async function removeFromLibrary(key: string): Promise<boolean> {
  const dialogs = useDialogsStore();
  const library = useLibraryStore();
  const sessions = useSessionsStore();
  const entry = library.repoByKey.get(key);
  const name = entry ? repoDisplayName(entry) : "this repository";
  const confirmed = await dialogs.open("confirm", {
    title: `Remove ${name} from the library?`,
    message: "Its tab closes and it leaves the Repositories list. The files on disk are not touched.",
    confirmLabel: "Remove",
    destructive: true,
  });
  if (!confirmed) return false;
  try {
    if (sessions.sessions.has(key)) await sessions.closeTab(key);
    await library.removeRepo(key);
    return true;
  } catch (e) {
    sessions.notify(errorMessage(e));
    return false;
  }
}

/** Why the working tree cannot be stashed right now, or null when it can. */
export function stashReason(session: RepoSession | null | undefined): string | null {
  if (!session) return "Open a repository first.";
  if (session.info.isBare) return "A bare repository has no working tree to stash.";
  const status = session.status;
  if (!status) return "The working tree status is still loading.";
  if (status.conflictedCount > 0) return "Resolve the conflicted files first: Git cannot stash them.";
  if (status.stagedCount + status.unstagedCount + status.untrackedCount === 0) return "There are no changes to stash.";
  return null;
}

/** Why there is nothing to apply right now, or null when there is at least one stash. */
export function applyStashReason(session: RepoSession | null | undefined): string | null {
  if (!session) return "Open a repository first.";
  if (session.info.isBare) return "A bare repository has no working tree to apply a stash to.";
  if ((session.stashes?.length ?? session.status?.stashCount ?? 0) === 0) return "There are no stashes to apply.";
  return null;
}

/** Opens the Stash Changes dialog, or explains why the working tree cannot be stashed. */
export async function openStashDialog(session: RepoSession | null): Promise<void> {
  const sessions = useSessionsStore();
  const reason = stashReason(session);
  if (reason || !session?.status) {
    sessions.notify(reason ?? "The working tree status is still loading.");
    return;
  }
  const status = session.status;
  await useDialogsStore().open("stashPush", {
    key: session.key,
    branch: currentBranch(session),
    trackedCount: status.stagedCount + status.unstagedCount,
    untrackedCount: status.untrackedCount,
  });
}

/** Opens the Apply Stash dialog, then applies or pops the stash picked there. */
export async function openApplyStashDialog(session: RepoSession | null): Promise<void> {
  const sessions = useSessionsStore();
  const reason = applyStashReason(session);
  if (reason || !session) {
    sessions.notify(reason ?? "Open a repository first.");
    return;
  }
  const choice = await useDialogsStore().open("stashApply", { key: session.key });
  if (!choice) return;
  if (choice.drop) await popStash(session.key, choice.stash);
  else await applyStash(session.key, choice.stash);
}

/**
 * Runs one stash operation and reports it. `conflictNote` is the notice when Git applied the
 * stash but stopped on conflicts, which is not a failure: the conflicted files are in the working
 * tree for the user to resolve.
 */
async function stashOp(
  key: string,
  op: Promise<IpcResult<OpResult>>,
  what: string,
  done: string,
  conflictNote: string,
): Promise<void> {
  const sessions = useSessionsStore();
  try {
    const result = await runOp(key, op);
    if (result.ok) sessions.notifySuccess(done);
    else if (result.conflicts) sessions.notify(conflictNote);
    else sessions.notify(opFailureText(result, what));
  } catch (e) {
    sessions.notify(errorMessage(e));
  }
}

/** Runs `git stash apply`: the stash's changes go into the working tree and the stash is kept. */
export async function applyStash(key: string, stash: StashEntry): Promise<void> {
  const name = stash.selector;
  await stashOp(
    key,
    commands.opStashApply(key, stash.index, stash.oid),
    `Applying ${name}`,
    `Applied ${name}. The stash is kept.`,
    `Applied ${name} with conflicts. Resolve them in the working tree; the stash is kept.`,
  );
}

/** Runs `git stash pop`: applies the stash, then drops it. Git keeps it when applying conflicts. */
export async function popStash(key: string, stash: StashEntry): Promise<void> {
  const name = stash.selector;
  await stashOp(
    key,
    commands.opStashPop(key, stash.index, stash.oid),
    `Popping ${name}`,
    `Applied and dropped ${name}.`,
    `Applying ${name} caused conflicts, so Git kept the stash. Resolve them, then drop it.`,
  );
}

/** Asks for confirmation, then runs `git stash drop`. */
export async function dropStash(key: string, stash: StashEntry, label: string): Promise<void> {
  const dialogs = useDialogsStore();
  const name = stash.selector;
  const confirmed = await dialogs.open("confirm", {
    title: `Drop ${name}?`,
    message: `${label}\n\nThe stash is deleted without being applied, and its changes are lost.`,
    confirmLabel: "Drop stash",
    destructive: true,
  });
  if (!confirmed) return;
  await stashOp(key, commands.opStashDrop(key, stash.index, stash.oid), `Dropping ${name}`, `Dropped ${name}.`, "");
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
