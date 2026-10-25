/**
 * Git write actions shared by the commit context menu, the branch context menu, the Branches
 * panel and the native menu bar, plus removing a repository from the library. Every function
 * reports through `sessions.notify`, refetches refs and status afterwards, and never throws.
 */
import {
  commands,
  type GitError,
  type GraphRow,
  type InProgress,
  type OpResult,
  type PullMode,
  type PushTarget,
  type RefEntry,
  type RefKind,
  type ResetMode,
  type ResolveSide,
  type StashEntry,
  type SyncResult,
  type TagsToPush,
} from "@/lib/bindings";
import { hasConflictMarkers, OPERATION_VERBS, operationNames, takeAllChoices, takeAllText, type OperationNames, type TakeAll } from "@/lib/conflicts";
import { pathBasename } from "@/lib/format";
import { mergeEditorReason } from "@/lib/fileViews";
import { commitFiles, selectedConflicts, worktreeActionPaths, type FileItem, type WorktreeActionPaths } from "@/lib/files";
import { openPath, revealItemInDir } from "@tauri-apps/plugin-opener";
import { errorMessage, IpcError, unwrap, type IpcResult } from "@/lib/ipc";
import { resolvedFileText } from "@/lib/merge/autoResolve";
import { shortSha } from "@/lib/refNames";
import { useDialogsStore, type BranchChoice, type PullSource, type PushDefaultChoice, type PushToServer, type RemoteDialogMode } from "@/stores/dialogs";
import { repoDisplayName, useLibraryStore } from "@/stores/library";
import { useSessionsStore, type NoticeAction, type RepoSession } from "@/stores/sessions";
import { useSettingsStore } from "@/stores/settings";
import { useUiStateStore } from "@/stores/uiState";

/**
 * Runs one write operation and refetches refs and status once it has finished, whatever the
 * outcome. `op` starts the command. When Git finds `index.lock` or `config.lock` still there (the
 * backend has already waited and tried once more), asks whether to remove it; if the user agrees,
 * removes it and runs the command once more. Otherwise the `indexLocked` or `configLocked` error
 * is thrown as it came.
 */
export async function runOp<T = OpResult>(key: string, op: () => Promise<IpcResult<T>>, ownOpId: string | null = null): Promise<T> {
  const sessions = useSessionsStore();
  // A write waits for a running pull, push or sync; if it is still waiting after a moment, the
  // notice says what for and offers to stop it. Not for a fetch: most writes go ahead beside one,
  // and the few that wait for it (deleting or renaming a branch or tag) would be told wrongly
  // that every slow write was waiting too.
  let waiting: string | null = null;
  const waitTimer = setTimeout(() => {
    const running = sessions.sessions.get(key)?.operation;
    if (!running || running.opId === ownOpId || running.command === "fetch") return;
    waiting = `Waiting for “${running.title}” to finish first.`;
    sessions.notify(waiting, running.cancellable ? { label: "Stop", run: () => stopOperation(key) } : null);
  }, 700);
  try {
    try {
      return await unwrap(op());
    } catch (e) {
      if (!(e instanceof IpcError && (e.error.kind === "indexLocked" || e.error.kind === "configLocked"))) throw e;
      const lock = e.error.kind === "indexLocked" ? "index" : "config";
      if (!(await confirmRemoveLock(e.error.lockPath, lock))) throw e;
      await unwrap(commands.opRemoveStaleLock(key, lock));
      return await unwrap(op());
    }
  } finally {
    clearTimeout(waitTimer);
    if (waiting !== null && sessions.notice === waiting) sessions.dismissNotice();
    void sessions.refetchRefsAndStatus(key);
  }
}

/** Asks whether to delete a lock file Git left behind: the index's, or the repository config's. */
function confirmRemoveLock(lockPath: string, lock: "index" | "config"): Promise<boolean> {
  const harm = lock === "index" ? "removing its lock can damage the index" : "removing its lock can lose the settings it is saving";
  return useDialogsStore().open("confirm", {
    title: "Remove Git's lock file?",
    message:
      `Git could not ${lock === "index" ? "run" : "save to the repository's settings file"} because its lock file is there: ${lockPath}\n\n` +
      "Git leaves it behind when one of its commands crashes or is killed. If no other program is running Git in this repository right now, " +
      `removing it is safe, and the command runs again. If another Git command is still running, wait for it instead: ${harm}.`,
    confirmLabel: "Remove lock and try again",
    destructive: true,
  });
}

/**
 * Makes `branch` track `upstream` (`refs/remotes/origin/dev`) as a step of its own, after a push
 * or a new branch that has already worked, and posts the outcome after `done` ("Pushed dev to
 * origin/dev."). Git writes the upstream into `.git/config`; when another Git process holds that
 * file (`config.lock`), `runOp` waits, then offers to remove a lock left behind. If it still
 * fails, the push or branch stands, and the notice says so and offers Track Remote Branch….
 */
async function trackAfter(key: string, branch: string, upstream: string, done: string): Promise<void> {
  const sessions = useSessionsStore();
  const short = upstream.replace(/^refs\/remotes\//, "");
  let failure: string;
  try {
    const result = await runOp(key, () => commands.opBranchSetUpstream(key, branch, upstream));
    if (result.ok) {
      sessions.notifySuccess(`${done} ${branch} now tracks ${short}.`);
      return;
    }
    failure = opFailureText(result, `Making ${branch} track ${short}`);
  } catch (e) {
    failure = errorMessage(e);
  }
  sessions.notify(`${done} But ${branch} does not track ${short} yet. ${failure}`, {
    label: "Track Remote Branch…",
    run: () => trackRemoteBranch(key, branch),
  });
}

/** One line for a Git command that ran but exited non-zero: `<what> failed (exit 1): <stderr>`. */
export function opFailureText(result: OpResult, what: string): string {
  const code = result.exitCode === null ? "" : ` (exit ${result.exitCode})`;
  const detail = gitReason(result.stderr) || result.stdout.trim();
  return detail.length > 0 ? `${what} failed${code}: ${detail}` : `${what} failed${code}.`;
}

/**
 * Git's progress lines, which a network command writes on stderr before any reason it fails:
 * `Enumerating objects: 7, done.`, `remote: Counting objects: …`, `Writing objects: …`, `Total …`,
 * and the `From …` or `To …` line that names the remote.
 */
const PROGRESS_LINE =
  /^(remote:\s*)?(Enumerating objects|Counting objects|Compressing objects|Writing objects|Receiving objects|Resolving deltas|Unpacking objects|Delta compression|Total \d+|POST git-|From |To )/;

/**
 * What Git says went wrong, without its progress lines, so a message leads with the reason
 * (`remote: main is a protected branch`, `! [remote rejected] …`, `fatal: …`) rather than with
 * how many objects it counted. All of `stderr` when it is nothing but progress.
 */
export function gitReason(stderr: string): string {
  const lines = stderr
    .split(/\r?\n|\r/)
    .map((l) => l.trim())
    .filter((l) => l.length > 0);
  const reasons = lines.filter((l) => !PROGRESS_LINE.test(l));
  return (reasons.length > 0 ? reasons : lines).join("\n");
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

/**
 * Why a merge is blocked right now: a merge, rebase, cherry-pick or revert is still in progress,
 * or the working tree has uncommitted changes. Null when neither.
 */
export function dirtyTreeReason(session: RepoSession | null | undefined): string | null {
  const summary = uncommittedSummary(session);
  return operationReason(session) ?? (summary === null ? null : `The working tree has uncommitted changes (${summary}). Commit or stash them first.`);
}

/** Why nothing else can start: a merge, rebase, cherry-pick or revert is in progress. Null when none is. */
export function operationReason(session: RepoSession | null | undefined): string | null {
  const op = session?.status?.inProgress;
  if (!op) return null;
  return `A ${OPERATION_VERBS[op.kind]} is in progress. Continue or abort it first, in the bar above the commits.`;
}

/** Where a notice about conflicts sends the user. */
const RESOLVE_HINT =
  "Resolve the files in the Conflicted section of the Files panel, then press Continue in the bar above the commits, or Abort there.";

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
 * What the Checkout of a branch's or tag's own menu goes to. A tag goes by its full name
 * (`refs/tags/v1`), so that a branch of the same name is not checked out instead.
 */
export function checkoutTargetOf(ref: { fullName: string; short: string; kind: RefKind }): CheckoutTarget {
  if (ref.kind.type === "remoteBranch") return { kind: "remote", remote: ref.short, local: stripRemote(ref.short, ref.kind.remote) };
  if (ref.kind.type === "localBranch") return { kind: "local", name: ref.short };
  return { kind: "revision", revision: ref.fullName, label: ref.short };
}

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
      // The full name, so that a tag or local branch also called `origin/dev` is not the start.
      // Created and checked out first, then made to track the remote branch as a step of its own
      // (`trackAfter`), so that a locked `.git/config` cannot leave it half made.
      const result = await runOp(key, () => commands.opBranchCreate(key, plan.local, `refs/remotes/${plan.remote}`, true));
      if (result.ok) await trackAfter(key, plan.local, `refs/remotes/${plan.remote}`, `Created branch ${plan.local} from ${plan.remote}, and checked it out.`);
      else sessions.notify(opFailureText(result, `Creating branch ${plan.local} from ${plan.remote}`));
      return;
    }
    const revision = plan.kind === "local" ? plan.name : plan.revision;
    const result = await runOp(key, () => commands.opCheckout(key, revision));
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
    const result = await runOp(key, () => commands.opMergeAbort(key));
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
    const result = await runOp(key, () => commands.opRebaseAbort(key));
    if (result.ok) sessions.notifySuccess("The rebase was aborted. The branch is back where it was before the rebase started.");
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
  return operationReason(session) ?? busyReason(session);
}

/**
 * Opens the Pull dialog for the checked-out branch, with `preferSource` (a remote branch's short
 * name, `origin/dev`) picked when there is one, else its upstream. When the working tree has
 * uncommitted changes, asks first; going ahead pulls with `--autostash`. The mode starts as the
 * one last used in this repository, else what the repository's Git config asks for, else Merge.
 * The dialog runs the pull itself and reports the outcome.
 */
export async function openPullDialog(session: RepoSession | null, preferSource: string | null = null): Promise<void> {
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
  const preferred = preferSource === null ? undefined : sources.find((s) => s.short === preferSource);
  const source = preferred ?? upstreamSource ?? sources.find((s) => s.short === `origin/${into}`) ?? sources[0];

  const autostash = await confirmAutostash(session, "Pull");
  if (autostash === null) return;
  const mode = await pullModeFor(key, into);
  await dialogs.open("pull", {
    key,
    into,
    sources,
    source,
    sourceIsUpstream: upstreamSource !== undefined && source === upstreamSource,
    mode,
    autostash,
  });
}

/**
 * Git's message when putting autostashed changes back conflicted, which it reports with exit code
 * 0 while keeping the changes as a stash. Git 2.47 and older print "Applying autostash resulted
 * in conflicts."; newer versions print "Your local changes are stashed, however applying them
 * resulted in conflicts." across two lines.
 */
const AUTOSTASH_CONFLICT = /Applying autostash resulted in conflicts|local changes are stashed, however applying them\s+resulted in conflicts/i;

/** How the uncommitted-changes question words each action that can stash them first. */
const AUTOSTASH_WORDS = {
  Pull: { doing: "Pulling", does: "pulls" },
  Sync: { doing: "Syncing", does: "pulls" },
  "Fast-forward": { doing: "Fast-forwarding", does: "fast-forwards" },
  Merge: { doing: "Merging", does: "merges" },
  Rebase: { doing: "Rebasing", does: "rebases" },
} as const;

/**
 * With uncommitted changes, asks whether to go ahead with `--autostash`. Resolves false when the
 * working tree is clean, true when the user chose to go ahead, and null when they cancelled.
 * `verb` names the action.
 */
async function confirmAutostash(session: RepoSession, verb: keyof typeof AUTOSTASH_WORDS): Promise<boolean | null> {
  const uncommitted = uncommittedSummary(session);
  if (uncommitted === null) return false;
  const words = AUTOSTASH_WORDS[verb];
  const confirmed = await useDialogsStore().open("confirm", {
    title: `${verb} with uncommitted changes?`,
    message:
      `The working tree has uncommitted changes (${uncommitted}). ${words.doing} anyway stashes them first, ${words.does}, ` +
      "then puts them back (--autostash). If putting them back conflicts, they also stay in a stash, so nothing is lost.",
    confirmLabel: `${verb} anyway (autostash)`,
  });
  return confirmed ? true : null;
}

/** The pull mode to use in a repository: the one last chosen there, else its Git config, else Merge. */
async function pullModeFor(key: string, branch: string): Promise<PullMode> {
  const remembered = useUiStateStore().state?.perRepo?.[key]?.pullMode ?? null;
  if (remembered !== null) return remembered;
  try {
    return (await unwrap(commands.getPullMode(key, branch))) ?? "merge";
  } catch {
    // Reading the config only chooses a default; Merge is a fine one.
    return "merge";
  }
}

/** What to report after a pull ran to the end, and the button an error notice offers. */
export interface PullOutcome {
  message: string;
  kind: "success" | "error";
  action: NoticeAction | null;
  /** A plain failure: the Pull dialog keeps it open so another mode or branch can be tried. */
  failed: boolean;
}

/** Turns a finished pull into the notice to show. `from` is `origin/dev`, `into` the local branch. */
export function pullOutcome(result: OpResult, from: string, into: string, mode: PullMode): PullOutcome {
  const output = `${result.stdout}\n${result.stderr}`;
  const leftBehind = result.ok ? null : updateRefsLeftBehind(output);
  if (leftBehind) {
    return {
      message: `Pulled ${from} into ${into}, with ${into}'s own commits rebased on top. ${updateRefsNote(leftBehind)}`,
      kind: "error",
      action: null,
      failed: false,
    };
  }
  if (result.ok && AUTOSTASH_CONFLICT.test(output)) {
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
  // The bar above the commits now shows the rebase or merge, with Continue and Abort.
  if (result.conflicts && mode === "rebase") {
    return { message: `The pull stopped on conflicts while rebasing ${into} onto ${from}. ${RESOLVE_HINT}`, kind: "error", action: null, failed: false };
  }
  if (result.conflicts) {
    return { message: `The pull stopped on conflicts while merging ${from} into ${into}. ${RESOLVE_HINT}`, kind: "error", action: null, failed: false };
  }
  return { message: networkFailureText(result, `Pulling ${from} into ${into}`), kind: "error", action: null, failed: true };
}

/**
 * `opFailureText` for a command that talks to a remote, with the one failure that needs
 * explaining: GIT_TERMINAL_PROMPT=0 stops Git asking for a username and password on a terminal it
 * does not have.
 */
export function networkFailureText(result: OpResult, what: string): string {
  if (/terminal prompts disabled|could not read Username/i.test(`${result.stdout}\n${result.stderr}`)) {
    return "Git needs a username and password for this remote, and StashPanda cannot ask for them yet. Set up a credential helper (Git Credential Manager on Windows), or use an SSH remote.";
  }
  return opFailureText(result, what);
}

/**
 * Repositories where a sync or push has been clicked and is still getting ready: reading the
 * upstream and the push target, and asking about uncommitted changes, before `runOperation` puts
 * it in the status bar. Without this a double click started two.
 */
const starting = new Set<string>();

/** Runs `action` with the repository marked busy from the first moment, so `busyReason` refuses a second one meanwhile. */
async function claimed(key: string, action: () => Promise<void>): Promise<void> {
  starting.add(key);
  try {
    await action();
  } finally {
    starting.delete(key);
  }
}

/** Why a fetch, pull or push cannot start because one is already running, or null. */
export function busyReason(session: RepoSession): string | null {
  const op = session.operation;
  if (op) return `Wait for the current operation to finish (${op.title.toLowerCase()}).`;
  return starting.has(session.key) ? "Wait for the current operation to finish." : null;
}

/**
 * Runs a fetch, pull or push and shows it in the status bar until it finishes: `start` gets the
 * operation id to pass to the command. `command` is the toolbar command whose button shows a
 * spinner meanwhile. Refetches refs and status afterwards. Throws what the command throws,
 * including `cancelled` after Stop.
 */
export async function runOperation<T = OpResult>(
  key: string,
  title: string,
  start: (opId: string) => Promise<IpcResult<T>>,
  command: "fetch" | "pull" | "push" | "sync" | null = null,
): Promise<T> {
  const sessions = useSessionsStore();
  const session = sessions.sessions.get(key);
  const opId = crypto.randomUUID();
  if (session) session.operation = { opId, title, line: "", cancellable: true, stopping: false, command };
  try {
    return await runOp(key, () => start(opId), opId);
  } finally {
    if (session && session.operation?.opId === opId) session.operation = null;
  }
}

/** Asks the running fetch, pull or push to stop. Its own call then fails with `cancelled`. */
export async function stopOperation(key: string): Promise<void> {
  const sessions = useSessionsStore();
  const session = sessions.sessions.get(key);
  const op = session?.operation;
  if (!session || !op || !op.cancellable || op.stopping) return;
  session.operation = { ...op, stopping: true };
  try {
    await unwrap(commands.opCancel(key, op.opId));
  } catch (e) {
    sessions.notify(errorMessage(e));
  }
}

function isCancelled(e: unknown): boolean {
  return e instanceof IpcError && e.error.kind === "cancelled";
}

/** Why the repository cannot fetch right now, or null when it can. */
export function fetchReason(session: RepoSession | null | undefined): string | null {
  if (!session) return "Open a repository first.";
  if (!session.refs) return "The repository is still loading.";
  if (session.refs.remotes.length === 0) return "This repository has no remotes to fetch from.";
  return busyReason(session);
}

/** Runs `git fetch --all`, with progress and Stop in the status bar. */
export async function fetchAll(session: RepoSession | null): Promise<void> {
  const sessions = useSessionsStore();
  const reason = fetchReason(session);
  if (reason || !session?.refs) {
    sessions.notify(reason ?? "The repository is still loading.");
    return;
  }
  const remotes = session.refs.remotes.map((r) => r.name);
  const from = remotes.length === 1 ? remotes[0] : `${remotes.length} remotes`;
  const key = session.key;
  try {
    const result = await runOperation(key, `Fetching from ${from}`, (opId) => commands.opFetch(key, opId), "fetch");
    session.lastFetchAt = Date.now();
    if (result.ok) {
      session.autoFetchFailed = false;
      sessions.notifySuccess(`Fetched from ${from}.`);
    } else {
      sessions.notify(networkFailureText(result, `Fetching from ${from}`));
    }
  } catch (e) {
    sessions.notify(isCancelled(e) ? "The fetch was stopped. The remote branches are as they were before it." : errorMessage(e));
  }
}

/** How long an automatic fetch may go without a progress line before it is stopped. */
const AUTO_FETCH_STALL_MS = 60_000;

/**
 * Fetches every open repository whose last fetch is `polling.autoFetchMinutes` old or more (the
 * time a tab opened counts as a fetch), one after another. Called every half minute by `App.vue`.
 * The fetch shows in the status bar like any other and can be stopped; a success says nothing,
 * since new commits show in the graph by themselves. A failure is reported once per repository,
 * until a fetch there works again, so a server that is down does not post a notice every few
 * minutes. Skips repositories with no remotes, still loading, or running a fetch, pull or push.
 * A fetch that reports no progress for `AUTO_FETCH_STALL_MS` is stopped, as a hung SSH remote
 * would otherwise keep it running, and the status bar busy, indefinitely.
 */
export async function autoFetchDue(): Promise<void> {
  const minutes = useSettingsStore().settings?.polling?.autoFetchMinutes ?? null;
  if (minutes === null || minutes <= 0) return;
  const sessions = useSessionsStore();
  for (const session of [...sessions.sessions.values()]) {
    if (Date.now() - session.lastFetchAt < minutes * 60_000 || fetchReason(session) !== null || !session.refs) continue;
    const key = session.key;
    const remotes = session.refs.remotes.map((r) => r.name);
    const from = remotes.length === 1 ? remotes[0] : `${remotes.length} remotes`;
    let lastLine: string | null = null;
    let lastChange = Date.now();
    let stalled = false;
    const watchdog = setInterval(() => {
      const line = session.operation?.command === "fetch" ? session.operation.line : null;
      if (line !== lastLine) {
        lastLine = line;
        lastChange = Date.now();
      } else if (Date.now() - lastChange >= AUTO_FETCH_STALL_MS && !stalled) {
        stalled = true;
        void stopOperation(key);
      }
    }, 5_000);
    try {
      const result = await runOperation(key, `Fetching from ${from} (automatic)`, (opId) => commands.opFetch(key, opId), "fetch");
      session.lastFetchAt = Date.now();
      if (result.ok) {
        session.autoFetchFailed = false;
      } else if (!session.autoFetchFailed) {
        session.autoFetchFailed = true;
        sessions.notify(`${networkFailureText(result, `Fetching ${session.info.displayName} automatically`)} Automatic fetches carry on; this is not reported again until one works.`);
      }
    } catch (e) {
      session.lastFetchAt = Date.now();
      if (stalled && !session.autoFetchFailed) {
        session.autoFetchFailed = true;
        sessions.notify(`Fetching ${session.info.displayName} automatically was stopped after a minute with no progress. Automatic fetches carry on; this is not reported again until one works.`);
      } else if (!stalled && !isCancelled(e) && !session.autoFetchFailed) {
        session.autoFetchFailed = true;
        sessions.notify(`Fetching ${session.info.displayName} automatically failed: ${errorMessage(e)}`);
      }
    } finally {
      clearInterval(watchdog);
    }
  }
}

/** Why the checked-out branch cannot be pushed right now, or null when it can. */
export function pushReason(session: RepoSession | null | undefined): string | null {
  if (!session) return "Open a repository first.";
  if (!session.status || !session.refs) return "The repository is still loading.";
  if (isDetached(session) || currentBranch(session) === null) return "HEAD is detached. Check out a branch to push it.";
  if (session.status.headOid === null) return "This branch has no commits yet, so there is nothing to push.";
  if (session.refs.remotes.length === 0) return "This repository has no remotes to push to.";
  return busyReason(session);
}

/**
 * Asks where to push `branch`, whose upstream has another name that Git's `push.default` does not
 * push to: there this time only, or always as a `push.default` saved for the repository, which
 * this saves before resolving. Null when cancelled, or when saving failed (with a notice).
 */
async function askWhereToPush(key: string, branch: string, upstream: PushTarget, action: "push" | "sync"): Promise<PushDefaultChoice | null> {
  const sessions = useSessionsStore();
  const choice = await useDialogsStore().open("pushDefault", { key, branch, upstream, action });
  if (choice?.kind !== "always") return choice;
  try {
    const saved = await runOp(key, () => commands.opSetPushDefault(key, choice.mode));
    if (saved.ok) return choice;
    sessions.notify(`The setting could not be saved: ${saved.stderr || saved.stdout}`);
  } catch (e) {
    sessions.notify(errorMessage(e));
  }
  return null;
}

/**
 * The notice for a push of `branch` to `target` that the remote refused because it has commits the
 * branch lacks. When they are missing because they were rewritten here (an amend or a rebase),
 * pulling cannot help and only a force push would replace them, so it says that and offers
 * nothing. Otherwise someone else pushed: for the checked-out branch, the button opens the Pull
 * dialog on `target` itself, which is not the upstream when the two differ; another branch has to
 * be checked out before anything can be pulled into it.
 */
async function rejectedPushNotice(key: string, branch: string, target: PushTarget): Promise<[string, NoticeAction | null]> {
  const sessions = useSessionsStore();
  const to = `${target.remote}/${target.branch}`;
  let rewritten = false;
  try {
    rewritten = await unwrap(commands.getRewrotePushedCommits(key, branch, target));
  } catch {
    // Only chooses the wording; the usual reading is safe.
  }
  if (rewritten) {
    return [
      `${to} refused the push: ${branch} no longer has commits that ${to} has, because they were rewritten here by an amend or a rebase. ` +
        `Replacing them on ${to} needs a force push. Pulling ${to} would bring the old copies back beside the new ones.`,
      forcePushAction(key, branch, target.remote, target.branch),
    ];
  }
  if (branch !== currentBranch(sessions.sessions.get(key))) {
    return [`${to} has commits that ${branch} does not have. Check out ${branch}, pull ${to} into it, then push again.`, null];
  }
  return [
    `${to} has commits that ${branch} does not have. Pull them first, then push again.`,
    { label: "Pull…", run: () => openPullDialog(sessions.sessions.get(key) ?? null, `${target.remote}/${target.branch}`) },
  ];
}

/** A notice's button that opens Push To for `branch`, to push it to a branch the user picks. */
function chooseWhereToPush(key: string, branch: string): NoticeAction {
  return { label: "Choose where…", run: () => pushBranchTo(key, branch) };
}

/** Why local branch `branch` cannot be pushed right now, or null when it can. */
export function pushBranchReason(session: RepoSession | null | undefined, branch: string): string | null {
  if (!session) return "Open a repository first.";
  if (branch === currentBranch(session)) return pushReason(session);
  if (!session.status || !session.refs) return "The repository is still loading.";
  if (session.refs.remotes.length === 0) return "This repository has no remotes to push to.";
  return busyReason(session);
}

/** Pushes the checked-out branch; see `pushBranch`. */
export async function pushCurrentBranch(session: RepoSession | null): Promise<void> {
  const reason = pushReason(session);
  const branch = currentBranch(session);
  if (reason || !session || branch === null) {
    useSessionsStore().notify(reason ?? "The repository is still loading.");
    return;
  }
  await pushBranch(session, branch);
}

/**
 * Pushes local `branch`, checked out or not, where plain `git push` would: the remote branch it
 * tracks, or where `remote.pushDefault`, `branch.<name>.pushRemote` or the remote's push refspecs
 * send it, with progress and Stop in the status bar. A branch that tracks nothing opens the Push
 * dialog to choose where, and to create that remote branch; it then tracks it, set as a step of
 * its own after the push (`trackAfter`), as `git push --set-upstream` would leave it untracking
 * without failing when `.git/config` is locked. One whose upstream has another name, when Git's
 * `push.default` does not push there, asks first (`askWhereToPush`), so that a branch started
 * from `origin/main` is never pushed onto `main` unasked. One that `git push` would send
 * somewhere StashPanda cannot push (a review ref such as `refs/for/main`) is not pushed; the
 * notice offers Push To instead. Never forces: when the remote has commits the branch lacks, the
 * notice offers to pull them.
 */
export async function pushBranch(session: RepoSession | null, branch: string): Promise<void> {
  const sessions = useSessionsStore();
  const reason = pushBranchReason(session, branch);
  if (reason || !session?.refs) {
    sessions.notify(reason ?? "The repository is still loading.");
    return;
  }
  await claimed(session.key, () => pushBranchNow(session, branch));
}

/** The rest of `pushBranch`, once the repository is marked busy. */
async function pushBranchNow(session: RepoSession, branch: string): Promise<void> {
  const sessions = useSessionsStore();
  if (!session.refs) return;
  const key = session.key;
  let target: PushTarget | null = null;
  let tracks: string | null = null;
  // Pushed where Git decides, the backend works the target out again; a chosen one is passed.
  let chosen = false;
  try {
    target = await unwrap(commands.getPushTarget(key, branch));
  } catch (e) {
    if (e instanceof IpcError && e.error.kind === "pushTargetRefused") {
      sessions.notify(errorMessage(e), chooseWhereToPush(key, branch));
      return;
    }
    if (!(e instanceof IpcError && e.error.kind === "invalidPath")) {
      sessions.notify(errorMessage(e));
      return;
    }
    // The upstream has another name and `push.default` does not push there.
    let upstream: PushTarget | null;
    try {
      upstream = await unwrap(commands.getUpstream(key, branch));
    } catch (e) {
      sessions.notify(errorMessage(e));
      return;
    }
    if (upstream === null) {
      sessions.notify(`${branch} cannot be pushed: ${e.error.reason}.`);
      return;
    }
    const choice = await askWhereToPush(key, branch, upstream, "push");
    if (!choice) return;
    if (choice.kind === "once") {
      target = upstream;
      chosen = true;
    } else if (choice.kind === "always") {
      // Saved, so the backend now pushes there by itself.
      target = choice.mode === "upstream" ? upstream : { remote: upstream.remote, branch };
    } else {
      target = null;
      tracks = `${upstream.remote}/${upstream.branch}`;
    }
  }
  let setUpstream = false;
  if (target === null) {
    chosen = true;
    const choice = await useDialogsStore().open("push", { key, branch, remotes: session.refs.remotes.map((r) => r.name), tracks });
    if (!choice) return;
    ({ target, setUpstream } = choice);
  }
  const to = `${target.remote}/${target.branch}`;
  try {
    const result = await runOperation(key, `Pushing ${branch} to ${to}`, (opId) => commands.opPush(key, opId, branch, chosen ? target : null), "push");
    const output = `${result.stdout}\n${result.stderr}`;
    if (result.ok) {
      const done = /Everything up-to-date/i.test(output) ? `${to} already has every commit on ${branch}.` : `Pushed ${branch} to ${to}.`;
      if (setUpstream) await trackAfter(key, branch, `refs/remotes/${target.remote}/${target.branch}`, done);
      else sessions.notifySuccess(done);
    } else if (result.stderr.includes("[rejected]")) {
      sessions.notify(...(await rejectedPushNotice(key, branch, target)));
    } else {
      sessions.notify(networkFailureText(result, `Pushing ${branch} to ${to}`));
    }
  } catch (e) {
    sessions.notify(
      isCancelled(e) ? `The push was stopped. ${to} may already have the commits; the next fetch shows whether it does.` : errorMessage(e),
    );
  }
}

/** Why the checked-out branch cannot be synced right now, or null when it can. */
export function syncReason(session: RepoSession | null | undefined): string | null {
  return pullReason(session) ?? pushReason(session);
}

/**
 * Pulls the checked-out branch's upstream into it, then pushes it back: one operation, with
 * progress and Stop in the status bar. The pull uses the mode last chosen in the Pull dialog,
 * else the Git config, else Merge; with uncommitted changes it asks first and goes ahead with
 * `--autostash`. A branch with no upstream has nothing to pull from, so this pushes it through
 * the Push dialog instead. Nothing is pushed when the pull fails or stops on conflicts.
 */
export async function syncCurrentBranch(session: RepoSession | null): Promise<void> {
  const sessions = useSessionsStore();
  const reason = syncReason(session);
  const branch = currentBranch(session);
  if (reason || !session || branch === null) {
    sessions.notify(reason ?? "The repository is still loading.");
    return;
  }
  await claimed(session.key, () => syncBranch(session, branch));
}

/** The rest of `syncCurrentBranch`, once the repository is marked busy. */
async function syncBranch(session: RepoSession, branch: string): Promise<void> {
  const sessions = useSessionsStore();
  const key = session.key;
  let upstream: PushTarget | null;
  try {
    upstream = await unwrap(commands.getUpstream(key, branch));
  } catch (e) {
    sessions.notify(errorMessage(e));
    return;
  }
  if (upstream === null) {
    // Publishing is the only half of a sync that applies.
    await pushCurrentBranch(session);
    return;
  }
  // Where the push goes when the user chose it for this sync only; null leaves it to Git.
  let pushTo: PushTarget | null = null;
  try {
    await unwrap(commands.getPushTarget(key, branch));
  } catch (e) {
    if (e instanceof IpcError && e.error.kind === "pushTargetRefused") {
      // Nothing has been pulled yet, and a sync cannot push where Git would; push it by itself.
      sessions.notify(`${errorMessage(e)} Nothing was synced.`, chooseWhereToPush(key, branch));
      return;
    }
    if (!(e instanceof IpcError && e.error.kind === "invalidPath")) {
      sessions.notify(errorMessage(e));
      return;
    }
    // The upstream has another name and Git's `push.default` does not push there. Nothing has
    // been pulled yet, so asking first changes nothing.
    const choice = await askWhereToPush(key, branch, upstream, "sync");
    if (!choice) return;
    if (choice.kind === "once") pushTo = upstream;
  }
  const autostash = await confirmAutostash(session, "Sync");
  if (autostash === null) return;
  const mode = await pullModeFor(key, branch);
  const to = `${upstream.remote}/${upstream.branch}`;
  try {
    const result = await runOperation(key, `Syncing ${branch} with ${to}`, (opId) => commands.opSync(key, opId, branch, mode, autostash, pushTo), "sync");
    const outcome = syncOutcome(key, result, branch, mode);
    if (outcome.kind === "success") sessions.notifySuccess(outcome.message);
    else sessions.notify(outcome.message, outcome.action);
  } catch (e) {
    sessions.notify(isCancelled(e) ? `The sync was stopped while fetching. Nothing was merged or pushed.` : errorMessage(e));
  }
}

/** Turns a finished sync into the notice to show. */
export function syncOutcome(key: string, result: SyncResult, branch: string, mode: PullMode): PullOutcome {
  const sessions = useSessionsStore();
  const to = `${result.target.remote}/${result.target.branch}`;
  // Where the push went, which differs from `to` only with push.default=current.
  const pushTo = `${result.pushTarget.remote}/${result.pushTarget.branch}`;
  const pulled = pullOutcome(result.pull, to, branch, mode);
  if (!result.pull.ok) {
    // Failed, or stopped on conflicts: the pull's own notice, which offers the abort.
    const suffix = pulled.failed ? "" : " Nothing was pushed.";
    return { ...pulled, message: pulled.message + suffix };
  }
  const upToDate = /Already up to date|Current branch .* is up to date/i.test(`${result.pull.stdout}\n${result.pull.stderr}`);
  // An autostash that conflicted on the way back keeps its warning in front of the push news.
  const pullPart = pulled.kind === "error" ? `${pulled.message} ` : upToDate ? "" : `${pulled.message} `;
  if (result.nothingToPush) {
    if (upToDate && pulled.kind !== "error") return { message: `${branch} is in sync with ${to}.`, kind: "success", action: null, failed: false };
    return { message: `${pullPart}There was nothing to push.`, kind: pulled.kind, action: null, failed: false };
  }
  if (result.pushStopped) {
    return {
      message: `${pullPart}The push was stopped, so ${pushTo} may not have ${branch}'s new commits yet; sync again to send them.`,
      kind: "error",
      action: null,
      failed: false,
    };
  }
  const push = result.push;
  if (push?.ok) return { message: `${pullPart}Pushed ${branch} to ${pushTo}.`, kind: pulled.kind, action: null, failed: false };
  if (push && push.stderr.includes("[rejected]") && result.rewrotePushed) {
    // Syncing again would rebase and be refused again; only a force push would replace them.
    return {
      message:
        `${pullPart}The rebase onto ${to} rewrote commits that ${pushTo} already had, so ${pushTo} refused the push. ` +
        `Replacing them on ${pushTo} needs a force push. To keep ${pushTo} as it is instead, reset ${branch} to ${pushTo}, then sync with Merge rather than Rebase.`,
      kind: "error",
      action: forcePushAction(key, branch, result.pushTarget.remote, result.pushTarget.branch),
      failed: false,
    };
  }
  if (push && push.stderr.includes("[rejected]") && pushTo !== to) {
    // The sync pulls the upstream, never the push target, so syncing again cannot bring these in.
    return {
      message: `${pullPart}${pushTo} has commits that ${branch} does not have, so the push was refused. Sync only pulls ${to}, so pull ${pushTo} into ${branch} first, then push again.`,
      kind: "error",
      action: { label: "Pull…", run: () => openPullDialog(sessions.sessions.get(key) ?? null, pushTo) },
      failed: false,
    };
  }
  if (push && push.stderr.includes("[rejected]")) {
    return {
      message: `${pullPart}${pushTo} received new commits while the sync was running, so the push was refused. Sync again.`,
      kind: "error",
      action: { label: "Sync again", run: () => syncCurrentBranch(sessions.sessions.get(key) ?? null) },
      failed: false,
    };
  }
  const detail = push ? networkFailureText(push, `Pushing ${branch} to ${pushTo}`) : `Pushing ${branch} to ${pushTo} did not run.`;
  return { message: `${pullPart}${detail}`, kind: "error", action: null, failed: false };
}

/**
 * Merges `target` (a branch name or an oid) into the checked-out branch; `noFf` makes a merge
 * commit even when a fast-forward would do. A conflicted merge stays open, and the bar above the
 * commits offers Continue and Abort.
 */
/**
 * How to name `ref` to Git as a revision to merge or rebase onto: its short name (`dev`,
 * `origin/dev`), which Git writes into a merge commit's message as given, unless another ref has
 * the same short name (a tag `dev` beside the branch `dev`). Then its full name, because Git
 * would pick the tag.
 */
export function revisionOf(ref: { fullName: string; short: string }, refs: readonly { fullName: string; short: string }[]): string {
  return refs.some((r) => r.short === ref.short && r.fullName !== ref.fullName) ? ref.fullName : ref.short;
}

export async function mergeInto(key: string, target: string, label: string = target, noFf = false): Promise<void> {
  const sessions = useSessionsStore();
  const session = sessions.sessions.get(key);
  const branch = currentBranch(session);
  const into = branch ?? "the current branch";
  // Uncommitted changes do not block a merge: it asks, then stashes them around it.
  const autostash = session ? await confirmAutostash(session, "Merge") : false;
  if (autostash === null) return;
  try {
    // The branch named here; the backend refuses when another has been checked out since.
    const result = await runOp(key, () => commands.opMerge(key, target, noFf, branch, autostash));
    const outcome = mergeOutcome(result, label, into, autostash);
    if (outcome.kind === "success") sessions.notifySuccess(outcome.message);
    else sessions.notify(outcome.message);
  } catch (e) {
    sessions.notify(errorMessage(e));
  }
}

/** What to report after a merge of `label` into `into` ran; `failed` is a plain failure, with nothing left open. */
export function mergeOutcome(result: OpResult, label: string, into: string, stashed = false): { message: string; kind: "success" | "error"; failed: boolean } {
  const output = `${result.stdout}\n${result.stderr}`;
  if (result.ok && AUTOSTASH_CONFLICT.test(output)) {
    return {
      message: `Merged ${label} into ${into}, but putting your uncommitted changes back caused conflicts. Resolve them in the working tree; your changes are also kept as stash@{0}.`,
      kind: "error",
      failed: false,
    };
  }
  if (result.ok) {
    const message = /Already up to date/i.test(output)
      ? `${into} already has every commit on ${label}.`
      : /Fast-forward/.test(output)
        ? `Fast-forwarded ${into} to ${label}.`
        : `Merged ${label} into ${into}.`;
    return { message, kind: "success", failed: false };
  }
  if (result.conflicts) {
    const aside = stashed ? " Your uncommitted changes are set aside, and come back when the merge is finished or aborted." : "";
    return { message: `Merging ${label} into ${into} stopped on conflicts. ${RESOLVE_HINT}${aside}`, kind: "error", failed: false };
  }
  return { message: opFailureText(result, `Merging ${label} into ${into}`), kind: "error", failed: true };
}

/** Where HEAD points, as messages name it: the checked-out branch, or "HEAD" when detached. */
function headName(session: RepoSession | null | undefined): string {
  return currentBranch(session) ?? "HEAD";
}

/** Why the branch cannot be reset right now, or null when it can. */
export function resetReason(session: RepoSession | null | undefined): string | null {
  if (!session) return "Open a repository first.";
  if (session.info.isBare) return "A bare repository has no checked-out branch to reset.";
  if (session.status?.headOid === null) return "This branch has no commits yet.";
  // A mixed or hard reset would end a merge or rebase silently, leaving its conflict markers.
  return operationReason(session);
}

const RESET_EFFECT: Record<ResetMode, string> = {
  soft: "Their changes stay staged, so committing again puts them back in one commit.",
  mixed: "Their changes, and anything staged now, become unstaged changes in the working tree. Nothing in the files is lost.",
  hard:
    "Every uncommitted change in the working tree and the index is lost, and so are the commits after it unless another branch or tag has them.",
};

/** Asks first, then runs `git reset --soft|--mixed|--hard <oid>`. */
export async function resetTo(key: string, oid: string, label: string, mode: ResetMode): Promise<void> {
  const sessions = useSessionsStore();
  const session = sessions.sessions.get(key);
  const reason = resetReason(session);
  if (reason) {
    sessions.notify(reason);
    return;
  }
  const name = headName(session);
  const head = session?.status?.headOid === oid;
  const confirmed = await useDialogsStore().open("confirm", {
    title: `${mode === "hard" ? "Hard reset" : mode === "soft" ? "Soft reset" : "Reset"} ${name} to ${label}?`,
    message: head
      ? `${name} stays at ${label}. ${mode === "hard" ? "Every uncommitted change in the working tree and the index is lost." : mode === "mixed" ? "Everything staged becomes unstaged. Nothing in the files is lost." : "Nothing changes."}`
      : `${name} moves to ${label}, and the commits after it are no longer on ${name}. ${RESET_EFFECT[mode]}`,
    confirmLabel: mode === "hard" ? "Reset and discard changes" : "Reset",
    destructive: mode === "hard",
  });
  if (!confirmed) return;
  try {
    // The branch the confirmation named; the backend refuses when another has been checked out since.
    const result = await runOp(key, () => commands.opReset(key, mode, oid, currentBranch(session)));
    if (result.ok) sessions.notifySuccess(`Reset ${name} to ${label} (${mode}).`);
    else sessions.notify(opFailureText(result, `Resetting ${name} to ${label}`));
  } catch (e) {
    sessions.notify(errorMessage(e));
  }
}

/** Why a commit cannot be cherry-picked or reverted onto HEAD right now, or null when it can. */
export function pickReason(session: RepoSession | null | undefined, row: GraphRow, verb: "cherry-pick" | "revert"): string | null {
  if (!session) return "Open a repository first.";
  if (session.info.isBare) return "A bare repository has no checked-out branch.";
  if (!session.status) return "The working tree status is still loading.";
  if (session.status.headOid === null) return "This branch has no commits yet.";
  const busy = operationReason(session);
  if (busy) return busy;
  if (session.status.conflictedCount > 0) return "Resolve the conflicted files first.";
  if (verb === "cherry-pick" && row.isHead) return "This commit is HEAD: its change is already there.";
  return null;
}

/**
 * Cherry-picks or reverts the commit on `row` onto HEAD. A merge commit is measured against its
 * first parent (`-m 1`). When Git stops on conflicts, the bar above the commits takes over with
 * Continue and Abort; when the change is already on the branch, the half-open cherry-pick is
 * aborted and that is said instead.
 */
export async function pickCommit(key: string, row: GraphRow, verb: "cherry-pick" | "revert"): Promise<void> {
  const sessions = useSessionsStore();
  const session = sessions.sessions.get(key);
  const reason = pickReason(session, row, verb);
  if (reason) {
    sessions.notify(reason);
    return;
  }
  const label = shortSha(row.oid);
  const name = headName(session);
  const mainline = row.parents.length > 1 ? 1 : null;
  const pick = verb === "cherry-pick";
  const abort = pick ? () => commands.opCherryPickAbort(key) : () => commands.opRevertAbort(key);
  const what = pick ? `Cherry-picking ${label} onto ${name}` : `Reverting ${label} on ${name}`;
  try {
    // The branch named here; the backend refuses when another has been checked out since.
    const onto = currentBranch(session);
    const result = await runOp(key, () => (pick ? commands.opCherryPick(key, row.oid, mainline, onto) : commands.opRevert(key, row.oid, mainline, onto)));
    const output = `${result.stdout}\n${result.stderr}`;
    if (result.ok) {
      sessions.notifySuccess(pick ? `Cherry-picked ${label} onto ${name}.` : `Reverted ${label} with a new commit on ${name}.`);
    } else if (/is now empty|nothing to commit/i.test(output)) {
      await runOp(key, abort);
      sessions.notify(pick ? `${name} already has the change ${label} made, so there was nothing to cherry-pick.` : `${name} no longer has the change ${label} made, so there was nothing to revert.`);
    } else if (result.conflicts || /--abort/.test(output)) {
      sessions.notify(`${what} stopped on conflicts. ${RESOLVE_HINT}`);
    } else {
      sessions.notify(opFailureText(result, what));
    }
  } catch (e) {
    sessions.notify(errorMessage(e));
  }
}

/** Runs `git cherry-pick --abort` or `git revert --abort` and reports the outcome. */
export async function abortPick(key: string, verb: "cherry-pick" | "revert"): Promise<void> {
  const sessions = useSessionsStore();
  try {
    const result = await runOp(key, () => (verb === "cherry-pick" ? commands.opCherryPickAbort(key) : commands.opRevertAbort(key)));
    if (result.ok) sessions.notifySuccess(`The ${verb} was aborted. The branch is back where it was.`);
    else sessions.notify(opFailureText(result, `Aborting the ${verb}`));
  } catch (e) {
    sessions.notify(errorMessage(e));
  }
}

// ---- The merge, rebase, cherry-pick or revert in progress ---------------------------------

/** Why Continue cannot run: files are still conflicted. Null when it can. */
export function continueReason(session: RepoSession | null | undefined): string | null {
  const status = session?.status;
  if (!status?.inProgress) return "No merge, rebase, cherry-pick or revert is in progress.";
  if (status.inProgress.interrupted) {
    return "Git was stopped part way through applying a commit, so continuing would leave that commit out. Abort puts the branch back as it was before the rebase.";
  }
  const n = status.conflictedCount;
  if (n === 0) return null;
  return `${n === 1 ? "1 file is" : `${n} files are`} still conflicted. Resolve ${n === 1 ? "it" : "each one"} first: take one side's version, or edit the file and mark it as resolved.`;
}

/**
 * Runs `git <merge|rebase|cherry-pick|revert> --continue`, which commits with the message Git
 * prepared. A rebase goes on to its next commit and may stop on conflicts again.
 */
export async function continueOperation(session: RepoSession | null): Promise<void> {
  const sessions = useSessionsStore();
  const op = session?.status?.inProgress ?? null;
  const reason = continueReason(session);
  if (reason || !session || !op) {
    sessions.notify(reason ?? "Open a repository first.");
    return;
  }
  const key = session.key;
  const names = operationNames(session.status, session.refs, session.stashConflict);
  try {
    const result = await runOp(key, () => commands.opContinue(key, op.kind));
    const outcome = continueOutcome(result, op, names);
    if (outcome.kind === "success") sessions.notifySuccess(outcome.message);
    else sessions.notify(outcome.message);
  } catch (e) {
    sessions.notify(errorMessage(e));
  }
}

/** What to report after `git <command> --continue` ran for the operation `op`. */
/**
 * The branches a rebase with `rebase.updateRefs` should also have moved at the end but could not,
 * because something else moved them while it ran, by their short names; null when `output` does
 * not say so. Git finishes the rebase ("Successfully rebased and updated refs/heads/main") and
 * then exits 1, listing them after "Failed to update the following refs with --update-refs:", so
 * the rebase is done although the command failed.
 */
export function updateRefsLeftBehind(output: string): string[] | null {
  if (!/Successfully rebased/i.test(output)) return null;
  const listed = /Failed to update the following refs with --update-refs:([\s\S]*)$/i.exec(output);
  if (!listed) return null;
  const refs = [...listed[1]!.matchAll(/refs\/heads\/(\S+)/g)].map((m) => m[1]!);
  return refs.length > 0 ? refs : null;
}

/** What to say about the branches `updateRefsLeftBehind` found. */
function updateRefsNote(refs: string[]): string {
  const one = refs.length === 1;
  return (
    `But ${refs.join(", ")} ${one ? "was" : "were"} not moved along with it, as rebase.updateRefs asks, because ${one ? "it" : "they"} changed ` +
    `while the rebase ran; ${one ? "it is" : "they are"} where that change left ${one ? "it" : "them"}.`
  );
}

export function continueOutcome(result: OpResult, op: InProgress, names: OperationNames): { message: string; kind: "success" | "error" } {
  const output = `${result.stdout}\n${result.stderr}`;
  const leftBehind = op.kind === "rebase" && !result.ok ? updateRefsLeftBehind(output) : null;
  if (leftBehind) return { message: `Finished rebasing onto ${names.ours}. ${updateRefsNote(leftBehind)}`, kind: "error" };
  const verb = OPERATION_VERBS[op.kind];
  const done =
    op.kind === "merge"
      ? `Merged ${names.theirs} into ${names.ours}`
      : op.kind === "rebase"
        ? `Finished rebasing onto ${names.ours}`
        : op.kind === "cherryPick"
          ? `Cherry-picked ${names.theirs} onto ${names.ours}`
          : `Reverted ${op.commit === null ? "the commit" : shortSha(op.commit)} with a new commit on ${names.ours}`;
  // A pull with --autostash: Git finishes, then putting the uncommitted changes back conflicts.
  // It still exits 0, and keeps the changes as stash@{0}.
  if (result.ok && AUTOSTASH_CONFLICT.test(output)) {
    return {
      message: `${done}, but putting your uncommitted changes back caused conflicts. Resolve them in the working tree; your changes are also kept as stash@{0}.`,
      kind: "error",
    };
  }
  if (result.ok) return { message: `${done}.`, kind: "success" };
  if (result.conflicts) return { message: `The rebase went on to the next commit, which conflicts too. ${RESOLVE_HINT}`, kind: "error" };
  if (/nothing to commit|No changes|is now empty/i.test(output)) {
    const way = op.kind === "rebase" || op.kind === "cherryPick" ? "Skip leaves the commit out." : `Abort the ${verb} to stop it.`;
    return { message: `${opFailureText(result, `Continuing the ${verb}`)} After resolving, nothing is left to commit. ${way}`, kind: "error" };
  }
  return { message: opFailureText(result, `Continuing the ${verb}`), kind: "error" };
}

/** Asks first, then leaves out the commit the rebase or cherry-pick stopped at (`--skip`). */
export async function skipOperation(session: RepoSession | null): Promise<void> {
  const sessions = useSessionsStore();
  const op = session?.status?.inProgress ?? null;
  if (!session || !op || (op.kind !== "rebase" && op.kind !== "cherryPick")) {
    sessions.notify("Only a rebase or a cherry-pick in progress can skip a commit.");
    return;
  }
  const names = operationNames(session.status, session.refs, session.stashConflict);
  const verb = OPERATION_VERBS[op.kind];
  const confirmed = await useDialogsStore().open("confirm", {
    title: `Skip ${names.theirs}?`,
    message:
      `The ${verb} leaves this commit out${op.subject ? ` ("${op.subject}")` : ""}, and its changes and any conflicts resolved so far are thrown away. ` +
      (op.kind === "rebase" ? "The rebase then goes on with the next commit." : "The cherry-pick then ends."),
    confirmLabel: "Skip commit",
    destructive: true,
  });
  if (!confirmed) return;
  const key = session.key;
  try {
    const result = await runOp(key, () => commands.opSkip(key, op.kind));
    if (result.ok) sessions.notifySuccess(op.kind === "rebase" ? `Skipped ${names.theirs}; the rebase onto ${names.ours} is finished.` : `Skipped ${names.theirs}.`);
    else if (result.conflicts) sessions.notify(`Skipped ${names.theirs}. The next commit conflicts too. ${RESOLVE_HINT}`);
    else sessions.notify(opFailureText(result, `Skipping ${names.theirs}`));
  } catch (e) {
    sessions.notify(errorMessage(e));
  }
}

/** Asks first, then aborts the operation in progress, which puts the branch back where it was. */
export async function abortOperation(session: RepoSession | null): Promise<void> {
  const op = session?.status?.inProgress ?? null;
  if (!session || !op) return;
  const verb = OPERATION_VERBS[op.kind];
  const head = op.kind === "rebase" ? (op.branch?.replace(/^refs\/heads\//, "") ?? "HEAD") : (currentBranch(session) ?? "HEAD");
  const confirmed = await useDialogsStore().open("confirm", {
    title: `Abort the ${verb}?`,
    message: `${head} goes back to where it was before the ${verb} started, and every conflict resolved so far is thrown away.`,
    confirmLabel: `Abort ${verb}`,
    destructive: true,
  });
  if (!confirmed) return;
  if (op.kind === "merge") await abortMerge(session.key);
  else if (op.kind === "rebase") await abortRebase(session.key);
  else await abortPick(session.key, op.kind === "cherryPick" ? "cherry-pick" : "revert");
}

// ---- Resolving conflicted files -------------------------------------------------------------

/** The selected rows of the Files panel's Conflicted section, when the Working Tree row is selected. */
export function selectedConflictItems(session: RepoSession | null | undefined): FileItem[] {
  if (!session || session.selectedRow?.kind !== "workingTree") return [];
  return selectedConflicts(session.status, session.fileSelection.ids);
}

/** Why conflicted files cannot be resolved from the selection, or null when they can. */
export function resolveReason(session: RepoSession | null | undefined, items: FileItem[]): string | null {
  if (!session) return "Open a repository first.";
  if (items.length === 0) return "Select conflicted files in the Files panel first.";
  return null;
}

/**
 * Resolves `items` by keeping one side's whole version and stages the result. `side` is in Git's
 * terms; the messages use the names from `operationNames`. A side that deleted a file deletes it.
 */
export async function takeVersion(session: RepoSession | null, items: FileItem[], side: ResolveSide): Promise<void> {
  const sessions = useSessionsStore();
  const reason = resolveReason(session, items);
  if (reason || !session) {
    sessions.notify(reason ?? "Open a repository first.");
    return;
  }
  const key = session.key;
  const names = operationNames(session.status, session.refs, session.stashConflict);
  const who = side === "ours" ? names.ours : names.theirs;
  const paths = items.map((i) => i.path);
  try {
    const result = await runOp(key, () => commands.opResolveConflicts(key, paths, side));
    if (result.ok) sessions.notifySuccess(`Resolved ${filesLabel(paths)} with the version from ${who}.`);
    else sessions.notify(opFailureText(result, `Taking the version from ${who} for ${filesLabel(paths)}`));
  } catch (e) {
    if (e instanceof IpcError && e.error.kind === "invalidPath") sessions.notify(`${e.error.path}: ${e.error.reason}.`);
    else sessions.notify(errorMessage(e));
  }
}

/**
 * Resolves every conflict block of `items` with one side's lines or both (`how`), keeping what Git
 * merged cleanly around the blocks, then marks the files resolved. Unlike `takeVersion`, which
 * replaces the whole file with one side's version, the other side's changes elsewhere stay.
 */
export async function takeAllConflicts(session: RepoSession | null, items: FileItem[], how: TakeAll): Promise<void> {
  const sessions = useSessionsStore();
  const reason = resolveReason(session, items) ?? items.map((i) => mergeEditorReason(i.letter)).find((r) => r !== null) ?? null;
  if (reason || !session) {
    sessions.notify(reason ?? "Open a repository first.");
    return;
  }
  const key = session.key;
  const written: string[] = [];
  for (const item of items) {
    try {
      const file = await unwrap(commands.getConflictFile(key, item.path));
      if (file.refused !== null) {
        sessions.notify(`${item.path} cannot be resolved block by block: take one side's version of it instead.`);
        continue;
      }
      await unwrap(commands.writeWorktreeFile(key, item.path, resolvedFileText(file, file.blocks.map((b) => takeAllText(b, how))), file.hash));
      written.push(item.path);
    } catch (e) {
      sessions.notify(`${item.path}: ${errorMessage(e)}`);
    }
  }
  if (written.length === 0) return;
  const choice = takeAllChoices(operationNames(session.status, session.refs, session.stashConflict)).find((c) => c.value === how);
  try {
    const result = await runOp(key, () => commands.opStage(key, written));
    if (result.ok) sessions.notifySuccess(`Resolved the conflicts in ${filesLabel(written)} with ${choice?.label ?? "the chosen lines"}.`);
    else sessions.notify(opFailureText(result, `Marking ${filesLabel(written)} resolved`));
  } catch (e) {
    sessions.notify(errorMessage(e));
  }
}

/**
 * Marks `items` as resolved as they are on disk (`git add`), after the user has edited them. When
 * a file still has Git's conflict markers, asks first.
 */
export async function markResolved(session: RepoSession | null, items: FileItem[]): Promise<void> {
  const sessions = useSessionsStore();
  const reason = resolveReason(session, items);
  if (reason || !session) {
    sessions.notify(reason ?? "Open a repository first.");
    return;
  }
  const key = session.key;
  const paths = items.map((i) => i.path);
  const marked: string[] = [];
  await Promise.all(
    paths.map(async (path) => {
      try {
        const file = await unwrap(commands.getWorktreeFileContent(key, path));
        if (!file.isBinary && hasConflictMarkers(file.text)) marked.push(path);
      } catch {
        // A file deleted to resolve the conflict has nothing to check.
      }
    }),
  );
  if (marked.length > 0) {
    const confirmed = await useDialogsStore().open("confirm", {
      title: "Mark as resolved with conflict markers?",
      message:
        `${marked.sort().slice(0, 8).join("\n")}${marked.length > 8 ? `\n…and ${marked.length - 8} more` : ""}\n\n` +
        `${marked.length === 1 ? "This file still has" : "These files still have"} lines Git added to mark the conflict (<<<<<<< and >>>>>>>). ` +
        "Marking it as resolved stages the file exactly as it is, markers included.",
      confirmLabel: "Mark as resolved",
      destructive: true,
    });
    if (!confirmed) return;
  }
  try {
    const result = await runOp(key, () => commands.opStage(key, paths));
    if (result.ok) sessions.notifySuccess(`Marked ${filesLabel(paths)} as resolved.`);
    else sessions.notify(opFailureText(result, `Marking ${filesLabel(paths)} as resolved`));
  } catch (e) {
    sessions.notify(errorMessage(e));
  }
}

// ---- Merge and Rebase dialogs ---------------------------------------------------------------

/** Every branch that could be merged into, or rebased onto from, the checked-out branch: local first, then remote. */
function otherBranches(session: RepoSession): BranchChoice[] {
  const current = currentBranch(session);
  const refs = session.refs?.refs ?? [];
  const local = refs.filter((r) => r.kind.type === "localBranch" && r.short !== current);
  const remote = refs.filter((r) => r.kind.type === "remoteBranch");
  return [...local, ...remote].map((r) => ({ short: r.short, revision: revisionOf(r, refs), remote: r.kind.type === "remoteBranch", target: r.target }));
}

/** Why the Merge or Rebase dialog cannot open now, or null when it can. */
export function mergeRebaseReason(session: RepoSession | null | undefined, action: "merge" | "rebase"): string | null {
  if (!session) return "Open a repository first.";
  if (session.info.isBare) return "A bare repository has no checked-out branch.";
  if (!session.status || !session.refs) return "The repository is still loading.";
  if (isDetached(session) || currentBranch(session) === null) {
    return action === "merge" ? "HEAD is detached. Check out a branch to merge into first." : "HEAD is detached. Check out the branch to rebase first.";
  }
  if (action === "rebase" && session.status.headOid === null) return "This branch has no commits yet, so there is nothing to rebase.";
  // Uncommitted changes do not block either: the dialog asks first, then stashes them around it.
  const blocked = operationReason(session);
  if (blocked) return blocked;
  if (otherBranches(session).length === 0) return `There is no other branch to ${action === "merge" ? "merge" : "rebase onto"}.`;
  return null;
}

/** Opens the Merge dialog to pick a branch to merge into the checked-out branch. The dialog runs the merge. */
export async function openMergeDialog(session: RepoSession | null): Promise<void> {
  const reason = mergeRebaseReason(session, "merge");
  const into = currentBranch(session);
  if (reason || !session || into === null) {
    useSessionsStore().notify(reason ?? "Open a repository first.");
    return;
  }
  const autostash = await confirmAutostash(session, "Merge");
  if (autostash === null) return;
  await useDialogsStore().open("merge", { key: session.key, into, branches: otherBranches(session), autostash });
}

/** Opens the Rebase dialog to pick a branch to rebase the checked-out branch onto. The dialog runs the rebase. */
export async function openRebaseDialog(session: RepoSession | null): Promise<void> {
  const reason = mergeRebaseReason(session, "rebase");
  const branch = currentBranch(session);
  if (reason || !session || branch === null) {
    useSessionsStore().notify(reason ?? "Open a repository first.");
    return;
  }
  const upstream = session.refs?.refs.find((r) => r.kind.type === "localBranch" && r.short === branch)?.upstream ?? null;
  const autostash = await confirmAutostash(session, "Rebase");
  if (autostash === null) return;
  await useDialogsStore().open("rebase", {
    key: session.key,
    branch,
    branches: otherBranches(session),
    upstream: upstream?.replace(/^refs\/remotes\//, "") ?? null,
    autostash,
  });
}

/**
 * What to report after `git rebase <onto>` ran; `failed` is a plain failure, with nothing left open.
 * `open` says the rebase is still in progress afterwards: Git can stop part way without a
 * conflict, as when an untracked file is in the way of a commit it replays.
 */
export function rebaseOutcome(
  result: OpResult,
  branch: string,
  onto: string,
  open = false,
  stashed = false,
): { message: string; kind: "success" | "error"; failed: boolean } {
  const output = `${result.stdout}\n${result.stderr}`;
  if (result.ok && AUTOSTASH_CONFLICT.test(output)) {
    return {
      message: `Rebased ${branch} onto ${onto}, but putting your uncommitted changes back caused conflicts. Resolve them in the working tree; your changes are also kept as stash@{0}.`,
      kind: "error",
      failed: false,
    };
  }
  const leftBehind = result.ok ? null : updateRefsLeftBehind(output);
  if (leftBehind) return { message: `Rebased ${branch} onto ${onto}. ${updateRefsNote(leftBehind)}`, kind: "error", failed: false };
  if (result.ok) {
    const message = /is up to date/i.test(output) ? `${branch} already has every commit on ${onto}.` : `Rebased ${branch} onto ${onto}.`;
    return { message, kind: "success", failed: false };
  }
  const aside = stashed ? " Your uncommitted changes are set aside, and come back when the rebase is finished or aborted." : "";
  if (result.conflicts) return { message: `Rebasing ${branch} onto ${onto} stopped on conflicts. ${RESOLVE_HINT}${aside}`, kind: "error", failed: false };
  if (open) {
    const text = opFailureText(result, `Rebasing ${branch} onto ${onto}`);
    return { message: `${text} The rebase stopped part way and is still open: fix what Git reports, then press Continue in the bar above the commits, or Abort there.`, kind: "error", failed: false };
  }
  return { message: opFailureText(result, `Rebasing ${branch} onto ${onto}`), kind: "error", failed: true };
}

/**
 * Opens the Rename Branch dialog for local branch `name`. The dialog renames it itself. When the
 * branch tracks a remote branch of the same name, the dialog offers to rename that too, which
 * `renameOnRemote` then does.
 */
export async function renameBranch(key: string, name: string): Promise<void> {
  const session = useSessionsStore().sessions.get(key);
  const ref = session?.refs?.refs.find((r) => r.kind.type === "localBranch" && r.short === name);
  const remote = ref?.upstream && !ref.upstreamGone ? session?.refs?.remotes.find((r) => ref.upstream === `refs/remotes/${r.name}/${name}`) : undefined;
  const remoteCopy = remote ? { remote: remote.name, branch: name } : null;
  const choice = await useDialogsStore().open("renameBranch", { key, name, remoteCopy });
  if (choice?.renameRemote && remoteCopy) await renameOnRemote(key, name, choice.name, remoteCopy.remote);
}

/**
 * The remote half of a rename, once local branch `old` is `renamed` and still tracks
 * `<remote>/<old>`: pushes it to a new branch `renamed` on `remote`, makes it track that, then
 * deletes `old` there (with the same lease as Delete Remote Branch…). Each step runs only when
 * the one before it worked, so `old` is never deleted before its commits are on `renamed`; the
 * notice says how far it got and offers the step that is left.
 */
export async function renameOnRemote(key: string, old: string, renamed: string, remote: string): Promise<void> {
  const sessions = useSessionsStore();
  const done = `Renamed branch ${old} to ${renamed}.`;
  const from = `${remote}/${old}`;
  const to = `${remote}/${renamed}`;
  const kept = `${from} is unchanged, and ${renamed} still tracks it.`;
  const pushing = `Pushing ${renamed} to ${to}`;
  try {
    const result = await runOperation(key, pushing, (opId) => commands.opPush(key, opId, renamed, { remote, branch: renamed }), "push");
    if (!result.ok) {
      const reason = result.stderr.includes("[rejected]")
        ? `${remote} already has a branch ${renamed} with commits that ${renamed} does not have, so the push was refused.`
        : networkFailureText(result, pushing);
      sessions.notify(`${done} ${reason} ${kept}`);
      return;
    }
  } catch (e) {
    sessions.notify(`${done} ${isCancelled(e) ? `The push to ${to} was stopped; the next fetch shows whether it arrived.` : errorMessage(e)} ${kept}`);
    return;
  }

  let trackFailure: string | null = null;
  try {
    const result = await runOp(key, () => commands.opBranchSetUpstream(key, renamed, `refs/remotes/${to}`));
    if (!result.ok) trackFailure = opFailureText(result, `Making ${renamed} track ${to}`);
  } catch (e) {
    trackFailure = errorMessage(e);
  }
  if (trackFailure !== null) {
    sessions.notify(`${done} Pushed it to ${to}, but it still tracks ${from}, so ${old} was not deleted on ${remote}. ${trackFailure}`, {
      label: "Track Remote Branch…",
      run: () => trackRemoteBranch(key, renamed),
    });
    return;
  }

  const deleting = `Deleting ${old} on ${remote}`;
  const left = `${done} Pushed it to ${to}, and it now tracks that, but ${from} is still there.`;
  const deleteAgain = { label: "Delete Remote Branch…", run: () => deleteRemoteBranch(key, from, remote) };
  try {
    const result = await runOperation(key, deleting, (opId) => commands.opDeleteRemoteBranch(key, opId, remote, old));
    if (result.ok || REMOTE_BRANCH_MISSING.test(`${result.stdout}\n${result.stderr}`)) {
      sessions.notifySuccess(`Renamed branch ${old} to ${renamed}, here and on ${remote}. ${renamed} now tracks ${to}.`);
    } else if (/refusing to delete the current branch/i.test(result.stderr)) {
      sessions.notify(
        `${left} ${remote} will not delete ${old} because it is the remote's default branch. Make ${renamed} the default branch in the repository's settings on the server, then delete ${from}.`,
        deleteAgain,
      );
    } else if (result.stderr.includes("(stale info)")) {
      sessions.notify(`${left} ${old} on ${remote} has commits that this repository has not fetched, so it was not deleted. Fetch and look at them first.`, deleteAgain);
    } else {
      sessions.notify(`${left} ${networkFailureText(result, deleting)}`, deleteAgain);
    }
  } catch (e) {
    sessions.notify(`${left} ${isCancelled(e) ? `The deletion was stopped; the next fetch shows whether ${remote} deleted it.` : errorMessage(e)}`, deleteAgain);
  }
}

/** Opens the Create Branch dialog starting at `startPoint`. The dialog runs the command itself. */
export async function createBranchHere(key: string, startPoint: string, startLabel: string, initialName?: string): Promise<void> {
  const dialogs = useDialogsStore();
  await dialogs.open("createBranch", { key, startPoint, startLabel, initialName });
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
  const op = session.status?.inProgress ?? null;
  if (op) {
    // A commit now finishes the operation, so it takes the index as it is.
    if (op.kind === "rebase") {
      sessions.notify("A rebase is in progress. Use Continue in the bar above to commit this step and go on.");
      return;
    }
    const conflictedCount = session.status?.conflictedCount ?? 0;
    if (conflictedCount > 0) {
      sessions.notify(`Resolve the conflicted files first (${conflictedCount} left), then commit or press Continue in the bar above.`);
      return;
    }
    const what = op.kind === "merge" ? "the merge" : op.kind === "cherryPick" ? "the cherry-pick" : "the revert";
    let message: string | null = null;
    try {
      message = await unwrap(commands.getPendingMessage(session.key));
    } catch {
      // Without Git's prepared message the user writes one.
    }
    const staged = files.filter((f) => f.staged);
    await dialogs.open("commit", {
      key: session.key,
      files: staged,
      ticked: staged.map((f) => f.path),
      fromSelection: false,
      branch: currentBranch(session),
      hasHead,
      finishing: { what, message },
    });
    return;
  }
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
    const result = await runOp(key, () => commands.opTagDelete(key, name));
    if (result.ok) sessions.notifySuccess(`Removed tag ${name}.`);
    else sessions.notify(opFailureText(result, `Removing tag ${name}`));
  } catch (e) {
    sessions.notify(errorMessage(e));
  }
}

/**
 * Why the remotes cannot be changed right now, or null when they can: `add` needs only a loaded
 * repository, editing, renaming or removing one needs a remote to act on.
 */
export function remoteReason(session: RepoSession | null | undefined, mode: RemoteDialogMode): string | null {
  if (!session) return "Open a repository first.";
  if (!session.refs) return "The repository is still loading.";
  if (mode !== "add" && session.refs.remotes.length === 0) return "This repository has no remotes.";
  return busyReason(session);
}

/**
 * Opens the Remote dialog to add a remote, or to edit, rename or remove remote `name` (null: picked
 * in the dialog, as from the menu bar). The dialog runs the command; after adding with "Fetch it
 * now" ticked, this fetches, with progress and Stop in the status bar.
 */
export async function openRemoteDialog(session: RepoSession | null | undefined, mode: RemoteDialogMode, name: string | null = null): Promise<void> {
  const sessions = useSessionsStore();
  const reason = remoteReason(session, mode);
  if (reason || !session?.refs) {
    sessions.notify(reason ?? "The repository is still loading.");
    return;
  }
  const result = await useDialogsStore().open("remote", { key: session.key, mode, remotes: session.refs.remotes, name });
  if (result?.fetch) {
    await fetchAll(session);
    // fetchAll reports a failure itself; on success, say what happened as a whole.
    if (sessions.successMessage?.startsWith("Fetched")) sessions.notifySuccess(`Added remote ${result.name}, and fetched it.`);
  }
}

/** Why tags cannot be pushed right now, or null when they can. */
export function pushTagsReason(session: RepoSession | null | undefined): string | null {
  if (!session) return "Open a repository first.";
  if (!session.refs) return "The repository is still loading.";
  if (session.refs.remotes.length === 0) return "This repository has no remotes to push to.";
  return busyReason(session);
}

/** The remote a tag goes to unless the user picks one: origin, else the first remote; null when there are none. */
export function defaultTagRemote(session: RepoSession | null | undefined): string | null {
  const remotes = session?.refs?.remotes.map((r) => r.name) ?? [];
  return remotes.includes("origin") ? "origin" : (remotes[0] ?? null);
}

/** What `opPushTags` returns as stdout when there was no tag to push (`NO_TAGS_TO_PUSH`). */
const NO_TAGS_TO_PUSH = "No tags to push.";

/** "v1", "v1 and v2", "v1, v2 and v3", or the first three and "and 4 more". */
function listNames(names: string[]): string {
  if (names.length <= 3) return names.length <= 1 ? (names[0] ?? "") : `${names.slice(0, -1).join(", ")} and ${names[names.length - 1]}`;
  return `${names.slice(0, 3).join(", ")} and ${names.length - 3} more`;
}

/**
 * What to report after `opPushTags`, from Git's own lines: ` * [new tag]  v1 -> v1` for a tag
 * pushed, ` ! [rejected]  v1 -> v1 (already exists)` for one the remote has on another commit.
 * `what` names the tags asked for ("v1.2", "every tag", "the tags on main").
 */
export function tagPushOutcome(result: OpResult, remote: string, what: string): { message: string; failed: boolean } {
  if (result.ok && result.stdout === NO_TAGS_TO_PUSH) return { message: `There were no tags to push to ${remote}.`, failed: false };
  const output = `${result.stdout}\n${result.stderr}`;
  const pushed = [...output.matchAll(/^\s*\*\s+\[new tag\]\s+(\S+)\s+->/gm)].map((m) => m[1]);
  const refused = [...output.matchAll(/^\s*!\s+\[rejected\]\s+(\S+)\s+->\s+\S+\s+\(already exists\)/gm)].map((m) => m[1]);
  const pushedNote = pushed.length > 0 ? ` Pushed ${listNames(pushed)}.` : "";
  if (result.ok) {
    return { message: pushed.length > 0 ? `Pushed ${listNames(pushed)} to ${remote}.` : `${remote} already has ${what}.`, failed: false };
  }
  if (refused.length > 0) {
    const them = refused.length === 1 ? `a tag ${refused[0]}` : `tags ${listNames(refused)}`;
    return {
      message:
        `${remote} already has ${them} on another commit, so ${refused.length === 1 ? "it was" : "they were"} not pushed: ` +
        `StashPanda never moves a tag on a remote, since others may already be using it.${pushedNote}`,
      failed: true,
    };
  }
  return { message: networkFailureText(result, `Pushing ${what} to ${remote}`), failed: true };
}

/**
 * Pushes `tags` to `remote`, with progress and Stop in the status bar, and reports the outcome.
 * `what` names them for the notices ("v1.2", "every tag"). Returns the outcome, or null when the
 * push could not start or was stopped (already reported).
 */
export async function pushTags(key: string, remote: string, tags: TagsToPush, what: string): Promise<{ message: string; failed: boolean } | null> {
  const sessions = useSessionsStore();
  const reason = pushTagsReason(sessions.sessions.get(key));
  if (reason) {
    sessions.notify(reason);
    return null;
  }
  try {
    const result = await runOperation(key, `Pushing ${what} to ${remote}`, (opId) => commands.opPushTags(key, opId, remote, tags), "push");
    const outcome = tagPushOutcome(result, remote, what);
    if (outcome.failed) sessions.notify(outcome.message);
    else sessions.notifySuccess(outcome.message);
    return outcome;
  } catch (e) {
    sessions.notify(isCancelled(e) ? `Pushing ${what} was stopped. ${remote} may already have some of the tags; the next fetch shows which.` : errorMessage(e));
    return null;
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
    let result = await runOp(key, () => commands.opBranchDelete(key, name, false));
    if (!result.ok && /not fully merged/i.test(result.stderr)) {
      const force = await dialogs.open("confirm", {
        title: `Force delete ${name}?`,
        message: `${result.stderr.trim()}\n\nDeleting it anyway makes those commits unreachable.`,
        confirmLabel: "Force delete",
        destructive: true,
      });
      if (!force) return;
      result = await runOp(key, () => commands.opBranchDelete(key, name, true));
    }
    if (result.ok) sessions.notifySuccess(`Deleted branch ${name}.`);
    else sessions.notify(opFailureText(result, `Deleting branch ${name}`));
  } catch (e) {
    sessions.notify(errorMessage(e));
  }
}

/**
 * Opens the Track Remote Branch dialog for local branch `name`, listing every remote branch with
 * the one it tracks now picked first, or, when it tracks nothing, the remote branch of the same
 * name. The dialog runs the command itself.
 */
export async function trackRemoteBranch(key: string, name: string): Promise<void> {
  const sessions = useSessionsStore();
  const refs = sessions.sessions.get(key)?.refs?.refs;
  if (!refs) {
    sessions.notify("The repository is still loading.");
    return;
  }
  const upstream = refs.find((r) => r.kind.type === "localBranch" && r.short === name)?.upstream ?? null;
  const choices = refs.flatMap((r) => (r.kind.type === "remoteBranch" ? [{ short: r.short, fullName: r.fullName, remote: r.kind.remote }] : []));
  if (choices.length === 0 && upstream === null) {
    sessions.notify(`This repository has no remote branches for ${name} to track. Fetch from a remote, or push ${name} to one, first.`);
    return;
  }
  // With no upstream, the remote branch of the same name is offered first, on origin if several
  // remotes have one: the usual case of a branch pushed without tracking, or fetched alongside.
  const sameName = choices.filter((c) => c.short === `${c.remote}/${name}`);
  const suggested = upstream === null ? (sameName.find((c) => c.remote === "origin") ?? sameName[0])?.fullName ?? null : null;
  await useDialogsStore().open("trackRemoteBranch", { key, branch: name, choices, upstream, suggested });
}

/**
 * What a remote says when asked to delete a branch it does not have. Git 2.43 sends the deletion
 * of a full ref name anyway, and the remote answers "warning: deleting a non-existent ref" and
 * succeeds; with a short name, or from some servers, Git fails with "remote ref does not exist".
 */
const REMOTE_BRANCH_MISSING = /deleting a non-existent ref|remote ref does not exist/i;

/** Why a remote branch cannot be deleted right now, or null when it can. */
export function deleteRemoteBranchReason(session: RepoSession | null | undefined): string | null {
  if (!session) return "Open a repository first.";
  return busyReason(session);
}

/**
 * Asks for confirmation, then deletes remote branch `short` (`origin/feature/x`) on `remote`
 * with `git push origin --delete refs/heads/feature/x`, with progress and Stop in the status bar.
 * Git also removes `origin/feature/x` from this repository. Local branches, including one that
 * tracks it, are left alone. When the remote's branch has moved on since the last fetch, nothing is
 * deleted (the backend pushes with a lease) and the notice says to fetch first.
 */
export async function deleteRemoteBranch(key: string, short: string, remote: string): Promise<void> {
  const sessions = useSessionsStore();
  const reason = deleteRemoteBranchReason(sessions.sessions.get(key));
  if (reason) {
    sessions.notify(reason);
    return;
  }
  const branch = stripRemote(short, remote);
  const confirmed = await useDialogsStore().open("confirm", {
    title: `Delete ${branch} on ${remote}?`,
    message:
      `The branch ${branch} is deleted on the remote ${remote}, for everyone who uses that remote, not only in this repository. ` +
      `Commits that no other branch there contains can then be lost.\n\nLocal branches, including any that track ${short}, are kept.`,
    confirmLabel: "Delete remote branch",
    destructive: true,
  });
  if (!confirmed) return;
  const what = `Deleting ${branch} on ${remote}`;
  try {
    const result = await runOperation(key, what, (opId) => commands.opDeleteRemoteBranch(key, opId, remote, branch));
    const missing = REMOTE_BRANCH_MISSING.test(`${result.stdout}\n${result.stderr}`);
    if (result.ok && missing) {
      sessions.notifySuccess(`${remote} no longer had a branch ${branch}, so there was nothing to delete there. ${short} has been removed from this repository.`);
    } else if (result.ok) {
      sessions.notifySuccess(`Deleted ${branch} on ${remote}.`);
    } else if (missing) {
      sessions.notify(`${remote} has no branch ${branch}, so there is nothing to delete. Fetch to bring the remote branches here up to date.`);
    } else if (result.stderr.includes("(stale info)")) {
      sessions.notify(
        `${branch} on ${remote} has commits that this repository has not fetched, so it was not deleted. ` +
          `Fetch, look at what is new on ${short}, then delete it again if it is still not needed.`,
      );
    } else {
      sessions.notify(networkFailureText(result, what));
    }
  } catch (e) {
    sessions.notify(isCancelled(e) ? `The deletion was stopped. ${remote} may already have deleted ${branch}; the next fetch shows whether it did.` : errorMessage(e));
  }
}

/** Why a branch cannot be fast-forwarded or pushed to another branch right now, or null when it can. */
export function fastForwardReason(session: RepoSession | null | undefined): string | null {
  if (!session) return "Open a repository first.";
  if (!session.refs) return "The repository is still loading.";
  return busyReason(session);
}

/**
 * Moves local branch `branch`, which is not checked out, forward to the checked-out branch with
 * `git fetch . <current>:<branch>`, without checking it out: the files, the index and the
 * checked-out branch stay as they are, so uncommitted changes are fine. The branch menu offers it
 * only for a branch on the current branch's own lane (`getFastForwardsToHead`); Git refuses when
 * it is no longer a fast-forward.
 */
export async function fastForwardToHead(key: string, branch: string): Promise<void> {
  const sessions = useSessionsStore();
  const session = sessions.sessions.get(key);
  const current = currentBranch(session);
  const reason = fastForwardReason(session);
  if (reason || current === null) {
    sessions.notify(reason ?? "HEAD is detached. Check out a branch first.");
    return;
  }
  try {
    const result = await runOp(key, () => commands.opBranchFastForward(key, branch, current));
    if (result.ok) sessions.notifySuccess(`Fast-forwarded ${branch} to ${current}.`);
    else sessions.notify(localPushFailure(result, current, branch, false), null);
  } catch (e) {
    sessions.notify(errorMessage(e));
  }
}

/**
 * The remote branch (`origin/main`) local branch `ref` can be fast-forwarded to, or null: it has
 * an upstream that is still there, has no commits of its own beyond it, and is behind it, all as
 * of the last fetch.
 */
export function upstreamFastForward(ref: Pick<RefEntry, "kind" | "upstream" | "upstreamGone" | "ahead" | "behind">): string | null {
  if (ref.kind.type !== "localBranch" || ref.upstreamGone || !ref.upstream?.startsWith("refs/remotes/")) return null;
  if (ref.ahead !== 0 || !ref.behind) return null;
  return ref.upstream.slice("refs/remotes/".length);
}

/**
 * Fast-forwards local branch `branch`, checked out or not, to its upstream `upstream`
 * (`origin/main`) as last fetched, without fetching. The checked-out branch's files change with
 * it, so with uncommitted changes this asks first and goes ahead with `--autostash`: stash them,
 * fast-forward, then put them back and drop the stash. Any other branch moves without touching
 * the files.
 */
export async function fastForwardToUpstream(key: string, branch: string, upstream: string): Promise<void> {
  const sessions = useSessionsStore();
  const session = sessions.sessions.get(key);
  const reason = fastForwardReason(session);
  if (reason || !session) {
    sessions.notify(reason ?? "Open a repository first.");
    return;
  }
  const checkedOut = currentBranch(session) === branch;
  const autostash = checkedOut ? await confirmAutostash(session, "Fast-forward") : false;
  if (autostash === null) return;
  const what = `Fast-forwarding ${branch} to ${upstream}`;
  try {
    const result = await runOp(key, () => commands.opBranchFastForwardToUpstream(key, branch, autostash));
    const output = `${result.stdout}\n${result.stderr}`;
    if (result.ok && AUTOSTASH_CONFLICT.test(output)) {
      sessions.notify(
        `Fast-forwarded ${branch} to ${upstream}, but putting your uncommitted changes back caused conflicts. Resolve them in the working tree; your changes are also kept as stash@{0}.`,
      );
    } else if (result.ok) {
      sessions.notifySuccess(`Fast-forwarded ${branch} to ${upstream}.`);
    } else if (/refusing to move [^:]+: (.+)$/m.test(result.stderr)) {
      sessions.notify(localPushFailure(result, upstream, branch, false));
    } else if (/Not possible to fast-forward|\[rejected\]|non-fast-forward/i.test(output)) {
      sessions.notify(`${branch} has commits that ${upstream} does not have, so it cannot be fast-forwarded. Pull or rebase it instead.`);
    } else {
      sessions.notify(opFailureText(result, what));
    }
  } catch (e) {
    sessions.notify(errorMessage(e));
  }
}

/** Where Push To starts: a server (by remote name, null for this repository), a branch there, and Force push. */
export interface PushToPreset {
  remote: string | null;
  target: string;
  force: boolean;
}

/**
 * Opens the Push To dialog for local branch `branch`: choose a server (this repository, or one of
 * its remotes) and one of the branches there, or name a new branch on a remote, and whether to
 * force. Then pushes: to a remote with
 * `git push` (forced with `--force-with-lease`, against the commit last fetched), or into a local
 * branch with `git fetch . <branch>:<target>`, which checks nothing out. Without Force push only a
 * fast-forward goes through. `preset` picks the server, branch and Force push the dialog starts
 * with; otherwise it starts on the branch's upstream, else the same name on origin. When the
 * dialog asks for it, a push to a remote then makes the branch track where it went, as a step of
 * its own (`trackAfter`).
 */
export async function pushBranchTo(key: string, branch: string, preset: PushToPreset | null = null): Promise<void> {
  const sessions = useSessionsStore();
  const session = sessions.sessions.get(key);
  const reason = fastForwardReason(session);
  if (reason || !session?.refs) {
    sessions.notify(reason ?? "The repository is still loading.");
    return;
  }
  const refs = session.refs.refs;
  const current = currentBranch(session);
  // During a rebase HEAD is detached, but the branch being rebased is just as much in use.
  const rebasing = session.status?.inProgress?.branch ?? null;
  const local = refs
    .filter((r) => r.kind.type === "localBranch" && r.short !== branch && r.short !== current && r.fullName !== rebasing)
    .map((r) => ({ name: r.short, oid: r.target }));
  const servers: PushToServer[] = [
    { remote: null, label: "This repository (local)", branches: local },
    ...session.refs.remotes.map((remote) => ({
      remote: remote.name,
      label: remote.name,
      branches: refs.flatMap((r) => {
        if (r.kind.type !== "remoteBranch" || r.kind.remote !== remote.name) return [];
        const name = stripRemote(r.short, remote.name);
        return name === "HEAD" ? [] : [{ name, oid: r.target }];
      }),
    })),
  ];
  const upstream = refs.find((r) => r.kind.type === "localBranch" && r.short === branch)?.upstream ?? null;
  const tracked = upstream === null ? null : servers.find((s) => s.remote !== null && upstream.startsWith(`refs/remotes/${s.remote}/`));
  const start = preset ?? {
    remote: tracked?.remote ?? (servers.some((s) => s.remote === "origin") ? "origin" : null),
    target: tracked && upstream ? upstream.slice(`refs/remotes/${tracked.remote}/`.length) : branch,
    force: false,
  };
  const server = Math.max(0, servers.findIndex((s) => s.remote === start.remote));
  const sourceOid = refs.find((r) => r.kind.type === "localBranch" && r.short === branch)?.target ?? null;
  if (sourceOid === null) {
    sessions.notify(`${branch} is not a branch here any more.`);
    return;
  }
  const choice = await useDialogsStore().open("pushTo", { key, branch, sourceOid, servers, server, target: start.target, force: start.force, upstream });
  if (!choice) return;
  const to = choice.remote === null ? choice.target : `${choice.remote}/${choice.target}`;
  const what = `${choice.force ? "Force-pushing" : "Pushing"} ${branch} to ${to}`;
  try {
    // A force push replaces the remote branch only if it is still at the commit whose dropped
    // commits the dialog listed, not wherever a background fetch has moved it since.
    const result = await runOperation(
      key,
      what,
      (opId) => commands.opPushTo(key, opId, branch, choice.remote, choice.target, choice.force, choice.expected, choice.source),
      "push",
    );
    const pushed = `${choice.force ? "Force-pushed" : "Pushed"} ${branch} to ${to}.`;
    // The tags on what was pushed go as a step of their own, so a refused tag is reported as such
    // and never as a refused branch.
    const tags = result.ok && choice.remote !== null && choice.includeTags ? await pushTagsAfter(key, choice.remote, choice.source, branch) : null;
    const done = tags && !tags.failed && tags.message ? `${pushed} ${tags.message}` : pushed;
    if (result.ok && choice.remote !== null && choice.track) {
      await trackAfter(key, branch, `refs/remotes/${to}`, done);
      if (tags?.failed) sessions.notify(`${pushed} ${tags.message}`);
    } else if (result.ok && tags?.failed) {
      sessions.notify(`${pushed} ${tags.message}`);
    } else if (result.ok) {
      sessions.notifySuccess(choice.remote === null ? `${choice.target} now points at ${branch}'s commit.` : done);
    } else if (result.stderr.includes("(source moved)")) {
      sessions.notify(`${branch} has moved since the list of dropped commits was made, so nothing was pushed. Open Push To again to see what would be dropped now.`);
    } else if (choice.remote === null && choice.force && /cannot lock ref|but expected/i.test(result.stderr)) {
      sessions.notify(`${choice.target} has moved since the list of dropped commits was made, so it was not moved. Open Push To again to see what would be dropped now.`);
    } else if (choice.remote === null) {
      // Forcing only helps a move that is not a fast-forward, not one into a branch in use.
      const forceHelps = !choice.force && result.stderr.includes("[rejected]");
      sessions.notify(localPushFailure(result, branch, choice.target, choice.force), forceHelps ? forcePushAction(key, branch, null, choice.target) : null);
    } else if (result.stderr.includes("(stale info)")) {
      sessions.notify(`${to} has moved since it was shown here, so the force push was refused and its new commits were kept. Fetch, look at them, then push again.`);
    } else if (result.stderr.includes("[rejected]")) {
      sessions.notify(...(await rejectedPushNotice(key, branch, { remote: choice.remote, branch: choice.target })));
    } else {
      sessions.notify(networkFailureText(result, what));
    }
  } catch (e) {
    sessions.notify(isCancelled(e) ? `The push was stopped. ${to} may already have the commits; the next fetch shows whether it does.` : errorMessage(e));
  }
}

/**
 * Pushes the tags on `commit`'s history to `remote` after `branch` was pushed there, as Push To's
 * "Also push its tags" does, and returns the outcome to add to the push's own notice. A stopped
 * or failed tag push becomes a failed outcome rather than its own notice.
 */
async function pushTagsAfter(key: string, remote: string, commit: string, branch: string): Promise<{ message: string; failed: boolean }> {
  const what = `the tags on ${branch}`;
  try {
    const result = await runOperation(key, `Pushing ${what} to ${remote}`, (opId) => commands.opPushTags(key, opId, remote, { type: "mergedInto", commit }), "push");
    const outcome = tagPushOutcome(result, remote, what);
    // Nothing new to say when there were none, or the remote had them all.
    if (!outcome.failed && !/^Pushed /.test(outcome.message)) return { message: "", failed: false };
    return outcome;
  } catch (e) {
    return { message: isCancelled(e) ? `Pushing its tags was stopped; ${remote} may already have some of them.` : `Its tags were not pushed: ${errorMessage(e)}`, failed: true };
  }
}

/** The notice for a push of local branch `source` into local branch `target` that Git refused. */
function localPushFailure(result: OpResult, source: string, target: string, forced: boolean): string {
  const detail = `${result.stderr}\n${result.stdout}`;
  const inUse = /refusing to move [^:]+: (.+)$/m.exec(result.stderr);
  if (inUse) return `${target} cannot be moved: ${inUse[1]}. Moving it now would leave that worktree out of step with it.`;
  if (/checked out at/i.test(detail)) {
    return `${target} is checked out, here or in another worktree, so Git will not move it: its files would no longer match it. Check out another branch there first.`;
  }
  if (!forced && detail.includes("[rejected]")) {
    return `${target} has commits that ${source} does not have, so it was not moved: only a fast-forward goes through without Force push.`;
  }
  return opFailureText(result, `Moving ${target} to ${source}`);
}

/** A notice's button that opens Push To with Force push ticked, on `remote` (null for this repository) and `target`. */
function forcePushAction(key: string, branch: string, remote: string | null, target: string): NoticeAction {
  return { label: "Force push…", run: () => pushBranchTo(key, branch, { remote, target, force: true }) };
}

/**
 * Opens a library entry in a tab, or makes it the active tab. A failure goes to
 * `reportOpenFailure`, which may ask the user to fix it and then open the repository again.
 * Resolves true once the tab is open.
 */
export async function openRepository(key: string): Promise<boolean> {
  try {
    await useSessionsStore().openTab(key);
    return true;
  } catch (e) {
    return reportOpenFailure(key, e);
  }
}

/**
 * Reports why a repository did not open. When Git refused the folder because another user owns
 * it, the notice offers "Trust this folder…". When SSH stopped it, a dialog asks for what is
 * missing (trust for an unknown host key, a key's passphrase, a password) and the repository is
 * opened again; a changed host key is shown as a warning with no way past it. Resolves true when
 * the repository opened after all.
 */
export async function reportOpenFailure(key: string, e: unknown): Promise<boolean> {
  const sessions = useSessionsStore();
  if (e instanceof IpcError && e.error.kind === "unsafeRepository") {
    const path = e.error.path;
    sessions.notify(`Git refuses to open ${path} because another user owns the folder.`, {
      label: "Trust this folder…",
      run: () => trustFolder(key, path),
    });
    return false;
  }
  if (e instanceof IpcError) {
    const entry = useLibraryStore().repoByKey.get(key);
    const fixed = await resolveSshProblem(e.error, entry ? repoDisplayName(entry) : "the repository");
    if (fixed) return openRepository(key);
    if (fixed === false) {
      // Cancelled: the notice keeps the reason, and a way back to the same question.
      const retry = e.error.kind === "sshHostKeyChanged" ? null : { label: "Try again…", run: () => void openRepository(key) };
      sessions.notify(errorMessage(e), retry);
      return false;
    }
  }
  sessions.notify(errorMessage(e));
  return false;
}

/** True when `reportOpenFailure` has a dialog or a notice button for `e`, rather than only its message. */
export function canResolveOpenFailure(e: unknown): boolean {
  if (!(e instanceof IpcError)) return false;
  return e.error.kind === "unsafeRepository" || sshProblemKind(e.error) !== null;
}

/** Which SSH dialog fixes `error`, or null when none does. */
export function sshProblemKind(error: GitError): "hostKey" | "passphrase" | "password" | null {
  switch (error.kind) {
    case "sshHostKeyUnknown":
    case "sshHostKeyChanged":
      return "hostKey";
    case "sshNeedsPassphrase":
    case "sshWrongPassphrase":
      return "passphrase";
    case "sshAuthFailed":
      // Without a user and port the failure came from Git's own ssh, which StashPanda cannot give a password to.
      return error.passwordAllowed && error.user !== null && error.port !== null ? "password" : null;
    default:
      return null;
  }
}

/**
 * Asks the user for what an SSH connection was missing: trust for an unknown host key (recorded
 * in known_hosts), a key's passphrase (remembered at least until StashPanda closes), or a
 * password (remembered until StashPanda closes). A changed host key gets a warning that only
 * closes. `subject` names what was being opened, such as the repository's name. Resolves true
 * when the connection can be tried again, false when the user cancelled or nothing can be done,
 * and null when `error` is not an SSH problem a dialog can fix.
 */
export async function resolveSshProblem(error: GitError, subject: string): Promise<boolean | null> {
  const dialogs = useDialogsStore();
  switch (error.kind) {
    case "sshHostKeyUnknown":
    case "sshHostKeyChanged": {
      const changed = error.kind === "sshHostKeyChanged";
      const trusted = await dialogs.open("sshHostKey", { host: error.host, port: error.port, fingerprint: error.fingerprint, changed, subject });
      return trusted && !changed;
    }
    case "sshNeedsPassphrase":
    case "sshWrongPassphrase": {
      const result = await dialogs.open("passphrase", {
        path: error.path,
        keyName: pathBasename(error.path),
        purpose: `to open ${subject}`,
        wrong: error.kind === "sshWrongPassphrase",
        forConnection: true,
      });
      return result !== null;
    }
    case "sshAuthFailed": {
      if (!error.passwordAllowed || error.user === null || error.port === null) return null;
      return dialogs.open("sshPassword", { host: error.host, port: error.port, user: error.user, rejected: error.passwordRejected, subject });
    }
    default:
      return null;
  }
}

/** Asks, then adds `path` to `safe.directory` in the global Git config and opens the repository. */
async function trustFolder(key: string, path: string): Promise<void> {
  const sessions = useSessionsStore();
  const confirmed = await useDialogsStore().open("confirm", {
    title: "Trust this folder?",
    message:
      `${path}\n\nGit only works in a repository owned by another user once you say you trust it, because a repository's settings can run programs. ` +
      "Trusting it adds the folder to safe.directory in your global Git config (git config --global --add safe.directory), for the Git that opens this repository. Only do this for a folder you know.",
    confirmLabel: "Trust and open",
  });
  if (!confirmed) return;
  try {
    const result = await unwrap(commands.repoFixSafeDirectory(key, path));
    if (!result.ok) {
      sessions.notify(opFailureText(result, "Adding the folder to safe.directory"));
      return;
    }
    if (await openRepository(key)) sessions.notifySuccess(`Git now trusts ${path}.`);
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
    message: "Are you sure you want to remove this repository from the list? The files on disk are not touched.",
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
  op: () => Promise<IpcResult<OpResult>>,
  what: string,
  done: string,
  conflictNote: string,
  /** The stash applied, which names the "Stashed changes" side of any conflicts. */
  stashName: string,
): Promise<void> {
  const sessions = useSessionsStore();
  try {
    const result = await runOp(key, op);
    if (result.ok) sessions.notifySuccess(done);
    else if (result.conflicts) {
      const session = sessions.sessions.get(key);
      if (session) session.stashConflict = stashName;
      sessions.notify(conflictNote);
    }
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
    () => commands.opStashApply(key, stash.index, stash.oid),
    `Applying ${name}`,
    `Applied ${name}. The stash is still kept.`,
    `Applied ${name} with conflicts. Resolve them in the working tree, the stash is still kept.`,
    name,
  );
}

/** Runs `git stash pop`: applies the stash, then drops it. Git keeps it when applying conflicts. */
export async function popStash(key: string, stash: StashEntry): Promise<void> {
  const name = stash.selector;
  await stashOp(
    key,
    () => commands.opStashPop(key, stash.index, stash.oid),
    `Popping ${name}`,
    `Applied and dropped ${name}.`,
    `Applying ${name} caused conflicts, so Git kept the stash. Resolve them, then drop it.`,
    name,
  );
}

/** Asks for confirmation, then runs `git stash drop`. */
export async function dropStash(key: string, stash: StashEntry, label: string): Promise<void> {
  const dialogs = useDialogsStore();
  const name = stash.selector;
  const confirmed = await dialogs.open("confirm", {
    title: `Drop ${name}?`,
    message: `${label}\n\nAre you sure you want to drop it? It will be deleted without being applied, and its changes will be lost.`,
    confirmLabel: "Drop stash",
    destructive: true,
  });
  if (!confirmed) return;
  await stashOp(key, () => commands.opStashDrop(key, stash.index, stash.oid), `Dropping ${name}`, `Dropped ${name}.`, "", name);
}

/** The working tree rows selected in the Files panel, sorted into what each file action would do. */
export function selectedWorktreePaths(session: RepoSession | null | undefined): WorktreeActionPaths {
  const ids = session?.selectedRow?.kind === "workingTree" ? session.fileSelection.ids : new Set<string>();
  return worktreeActionPaths(session?.status ?? null, ids);
}

/** "notes.txt" for one file, "3 files" for more. */
export function filesLabel(paths: string[]): string {
  return paths.length === 1 ? (paths[0] ?? "1 file") : `${paths.length} files`;
}

/** Why the selected files cannot be staged, unstaged or discarded, or null when they can. */
export function fileActionReason(session: RepoSession | null | undefined, action: "stage" | "unstage" | "discard"): string | null {
  if (!session) return "Open a repository first.";
  if (session.info.isBare) return "A bare repository has no working tree to work on.";
  if (session.selectedRow?.kind !== "workingTree") return `Select the Working Tree at the top of the list first, then select which files you'd like to ${action}.`;
  const p = selectedWorktreePaths(session);
  const files = action === "stage" ? p.stageFiles : action === "unstage" ? p.unstageFiles : p.discardFiles;
  if (files.length > 0) return null;
  if (session.fileSelection.ids.size === 0) return "Select files in the Files panel first.";
  return action === "stage"
    ? "None of the selected files has unstaged changes."
    : action === "unstage"
      ? "None of the selected files is staged."
      : "None of the selected files has changes to discard.";
}

/** Stages the selected unstaged and untracked files. */
export async function stageSelected(session: RepoSession | null): Promise<void> {
  const sessions = useSessionsStore();
  const reason = fileActionReason(session, "stage");
  if (reason || !session) {
    sessions.notify(reason ?? "Please open a repository first.");
    return;
  }
  const p = selectedWorktreePaths(session);
  const key = session.key;
  try {
    const result = await runOp(key, () => commands.opStage(key, p.stage));
    if (result.ok) sessions.notifySuccess(`Staged ${filesLabel(p.stageFiles)}.`);
    else sessions.notify(opFailureText(result, `Staging ${filesLabel(p.stageFiles)}`));
  } catch (e) {
    sessions.notify(errorMessage(e));
  }
}

/** Unstages the selected staged files; their changes stay in the working tree. */
export async function unstageSelected(session: RepoSession | null): Promise<void> {
  const sessions = useSessionsStore();
  const reason = fileActionReason(session, "unstage");
  if (reason || !session) {
    sessions.notify(reason ?? "Please open a repository first.");
    return;
  }
  const p = selectedWorktreePaths(session);
  const key = session.key;
  try {
    const result = await runOp(key, () => commands.opUnstage(key, p.unstage));
    if (result.ok) sessions.notifySuccess(`Unstaged ${filesLabel(p.unstageFiles)}. The changes are still in the working tree.`);
    else sessions.notify(opFailureText(result, `Unstaging ${filesLabel(p.unstageFiles)}`));
  } catch (e) {
    sessions.notify(errorMessage(e));
  }
}

/**
 * Asks first, then throws away the selected files' changes: an unstaged row goes back to the
 * staged version, a staged row back to the last commit (its unstaged changes go too), and an
 * untracked file is deleted.
 */
export async function discardSelected(session: RepoSession | null): Promise<void> {
  const sessions = useSessionsStore();
  const reason = fileActionReason(session, "discard");
  if (reason || !session) {
    sessions.notify(reason ?? "Open a repository first.");
    return;
  }
  const p = selectedWorktreePaths(session);
  const key = session.key;
  const shown = p.discardFiles.slice(0, 8).join("\n");
  const more = p.discardFiles.length > 8 ? `\n…and ${p.discardFiles.length - 8} more` : "";
  const parts: string[] = [];
  if (p.discardToIndex.length > 0) parts.push("Unstaged changes go back to the staged version, or to the last commit when nothing is staged.");
  if (p.discardToHead.length > 0) parts.push("Staged files go back to the last commit, together with any unstaged changes to them.");
  if (p.discardUntracked.length > 0) parts.push("Untracked files are deleted.");
  const confirmed = await useDialogsStore().open("confirm", {
    title: `Discard changes to ${filesLabel(p.discardFiles)}?`,
    message: `${shown}${more}\n\n${parts.join(" ")} This cannot be undone.`,
    confirmLabel: "Discard changes",
    destructive: true,
  });
  if (!confirmed) return;
  const what = `Discarding changes to ${filesLabel(p.discardFiles)}`;
  // The staged files go in a first step of their own. When the second step fails, they are gone
  // already, and the notice says so rather than reporting the whole discard as failed.
  let headDone = false;
  const staged = p.discardToHead.length === 1 ? `${p.discardToHead[0]}, which was staged,` : `the ${p.discardToHead.length} staged files`;
  const partly = (failure: string): string =>
    headDone ? `${failure} The changes to ${staged} had already been discarded; the other files were left as they were.` : failure;
  try {
    if (p.discardToHead.length > 0) {
      const r = await runOp(key, () => commands.opDiscard(key, p.discardToHead, [], true));
      if (!r.ok) {
        sessions.notify(opFailureText(r, what));
        return;
      }
      headDone = true;
    }
    if (p.discardToIndex.length > 0 || p.discardUntracked.length > 0) {
      const r = await runOp(key, () => commands.opDiscard(key, p.discardToIndex, p.discardUntracked, false));
      if (!r.ok) {
        sessions.notify(partly(opFailureText(r, what)));
        return;
      }
    }
    sessions.notifySuccess(`Discarded the changes to ${filesLabel(p.discardFiles)}.`);
  } catch (e) {
    sessions.notify(partly(errorMessage(e)));
  }
}

/** Where the app can open `path` (relative to the top of the working tree), or null over SSH. */
async function hostPathOf(key: string, path: string): Promise<string | null> {
  return unwrap(commands.getHostPath(key, path));
}

/** Why a file of this repository cannot be opened or revealed on this computer, or null. */
export function openFileReason(session: RepoSession | null | undefined): string | null {
  if (!session) return "Open a repository first.";
  if (session.info.transport.type === "ssh") return "The file is on another computer, reached over SSH.";
  return null;
}

/** Opens a working tree file with the program the system uses for its type. */
export async function openWorktreeFile(key: string, path: string): Promise<void> {
  const sessions = useSessionsStore();
  try {
    const host = await hostPathOf(key, path);
    if (host === null) {
      sessions.notify("The file is on another computer, reached over SSH, so it cannot be opened here.");
      return;
    }
    await openPath(host);
  } catch (e) {
    sessions.notify(`Could not open ${path}: ${errorMessage(e)}`);
  }
}

/** Shows a working tree file in the system's file manager. */
export async function revealWorktreeFile(key: string, path: string): Promise<void> {
  const sessions = useSessionsStore();
  try {
    const host = await hostPathOf(key, path);
    if (host === null) {
      sessions.notify("The file is on another computer, reached over SSH, so it cannot be shown here.");
      return;
    }
    await revealItemInDir(host);
  } catch (e) {
    sessions.notify(`Could not show ${path} in its folder: ${errorMessage(e)}`);
  }
}

/** Copies the file's full path as this computer sees it (the path inside the repository over SSH). */
export async function copyFullPath(session: RepoSession, path: string): Promise<void> {
  try {
    const host = await hostPathOf(session.key, path);
    await copyText(host ?? `${session.info.toplevel.replace(/[/\\]+$/, "")}/${path}`, "the path");
  } catch (e) {
    useSessionsStore().notify(errorMessage(e));
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
