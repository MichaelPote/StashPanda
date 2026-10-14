/**
 * Git write actions shared by the commit context menu, the branch context menu, the Branches
 * panel and the native menu bar, plus removing a repository from the library. Every function
 * reports through `sessions.notify`, refetches refs and status afterwards, and never throws.
 */
import {
  commands,
  type GitError,
  type GraphRow,
  type OpResult,
  type PullMode,
  type PushTarget,
  type ResetMode,
  type ResolveSide,
  type StashEntry,
  type SyncResult,
} from "@/lib/bindings";
import { hasConflictMarkers, OPERATION_VERBS, operationNames } from "@/lib/conflicts";
import { pathBasename } from "@/lib/format";
import { commitFiles, selectedConflicts, worktreeActionPaths, type FileItem, type WorktreeActionPaths } from "@/lib/files";
import { openPath, revealItemInDir } from "@tauri-apps/plugin-opener";
import { errorMessage, IpcError, unwrap, type IpcResult } from "@/lib/ipc";
import { shortSha } from "@/lib/refNames";
import { useDialogsStore, type BranchChoice, type PullSource } from "@/stores/dialogs";
import { repoDisplayName, useLibraryStore } from "@/stores/library";
import { useSessionsStore, type NoticeAction, type RepoSession } from "@/stores/sessions";
import { useSettingsStore } from "@/stores/settings";
import { useUiStateStore } from "@/stores/uiState";

/**
 * Runs one write operation and refetches refs and status once it has finished, whatever the
 * outcome. `op` starts the command. When Git finds `index.lock` still there (the backend has
 * already waited and tried once more), asks whether to remove it; if the user agrees, removes it
 * and runs the command once more. Otherwise the `indexLocked` error is thrown as it came.
 */
export async function runOp<T = OpResult>(key: string, op: () => Promise<IpcResult<T>>): Promise<T> {
  const sessions = useSessionsStore();
  try {
    try {
      return await unwrap(op());
    } catch (e) {
      if (!(e instanceof IpcError && e.error.kind === "indexLocked")) throw e;
      if (!(await confirmRemoveLock(e.error.lockPath))) throw e;
      await unwrap(commands.opRemoveStaleLock(key));
      return await unwrap(op());
    }
  } finally {
    void sessions.refetchRefsAndStatus(key);
  }
}

/** Asks whether to delete a lock file Git left behind. */
function confirmRemoveLock(lockPath: string): Promise<boolean> {
  return useDialogsStore().open("confirm", {
    title: "Remove Git's lock file?",
    message:
      `Git did not run because its lock file is there: ${lockPath}\n\n` +
      "Git leaves it behind when one of its commands crashes or is killed. If no other program is running Git in this repository right now, " +
      "removing it is safe, and the command runs again. If another Git command is still running, wait for it instead: removing its lock can damage the index.",
    confirmLabel: "Remove lock and try again",
    destructive: true,
  });
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
      const result = await runOp(key, () => commands.opBranchCreate(key, plan.local, plan.remote, true));
      if (result.ok) sessions.notifySuccess(`Created branch ${plan.local} tracking ${plan.remote}, and checked it out.`);
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
 * With uncommitted changes, asks whether to go ahead with `--autostash`. Resolves false when the
 * working tree is clean, true when the user chose to go ahead, and null when they cancelled.
 * `verb` names the action: "Pull" or "Sync".
 */
async function confirmAutostash(session: RepoSession, verb: string): Promise<boolean | null> {
  const uncommitted = uncommittedSummary(session);
  if (uncommitted === null) return false;
  const confirmed = await useDialogsStore().open("confirm", {
    title: `${verb} with uncommitted changes?`,
    message:
      `The working tree has uncommitted changes (${uncommitted}). ${verb === "Pull" ? "Pulling" : "Syncing"} anyway stashes them first, pulls, ` +
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

/** Why a fetch, pull or push cannot start because one is already running, or null. */
function busyReason(session: RepoSession): string | null {
  const op = session.operation;
  return op ? `Wait for the current operation to finish (${op.title.toLowerCase()}).` : null;
}

/**
 * Runs a fetch, pull or push and shows it in the status bar until it finishes: `start` gets the
 * operation id to pass to the command. Refetches refs and status afterwards. Throws what the
 * command throws, including `cancelled` after Stop.
 */
export async function runOperation<T = OpResult>(
  key: string,
  title: string,
  start: (opId: string) => Promise<IpcResult<T>>,
): Promise<T> {
  const sessions = useSessionsStore();
  const session = sessions.sessions.get(key);
  const opId = crypto.randomUUID();
  if (session) session.operation = { opId, title, line: "", cancellable: true, stopping: false };
  try {
    return await runOp(key, () => start(opId));
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
    const result = await runOperation(key, `Fetching from ${from}`, (opId) => commands.opFetch(key, opId));
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

/**
 * Fetches every open repository whose last fetch is `polling.autoFetchMinutes` old or more (the
 * time a tab opened counts as a fetch), one after another. Called every half minute by `App.vue`.
 * The fetch shows in the status bar like any other and can be stopped; a success says nothing,
 * since new commits show in the graph by themselves. A failure is reported once per repository,
 * until a fetch there works again, so a server that is down does not post a notice every few
 * minutes. Skips repositories with no remotes, still loading, or running a fetch, pull or push.
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
    try {
      const result = await runOperation(key, `Fetching from ${from} (automatic)`, (opId) => commands.opFetch(key, opId));
      session.lastFetchAt = Date.now();
      if (result.ok) {
        session.autoFetchFailed = false;
      } else if (!session.autoFetchFailed) {
        session.autoFetchFailed = true;
        sessions.notify(`${networkFailureText(result, `Fetching ${session.info.displayName} automatically`)} Automatic fetches carry on; this is not reported again until one works.`);
      }
    } catch (e) {
      session.lastFetchAt = Date.now();
      if (!isCancelled(e) && !session.autoFetchFailed) {
        session.autoFetchFailed = true;
        sessions.notify(`Fetching ${session.info.displayName} automatically failed: ${errorMessage(e)}`);
      }
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
 * Pushes the checked-out branch to the remote branch it tracks, with progress and Stop in the
 * status bar. A branch that tracks nothing opens the Push dialog to choose where, which then
 * sets that as its upstream. Never forces: when the remote has commits the branch lacks, the
 * notice offers to pull them.
 */
export async function pushCurrentBranch(session: RepoSession | null): Promise<void> {
  const sessions = useSessionsStore();
  const reason = pushReason(session);
  const branch = currentBranch(session);
  if (reason || !session?.refs || branch === null) {
    sessions.notify(reason ?? "The repository is still loading.");
    return;
  }
  const key = session.key;
  let target: PushTarget | null;
  try {
    target = await unwrap(commands.getUpstream(key, branch));
  } catch (e) {
    sessions.notify(errorMessage(e));
    return;
  }
  let setUpstream = false;
  if (target === null) {
    const choice = await useDialogsStore().open("push", { key, branch, remotes: session.refs.remotes.map((r) => r.name) });
    if (!choice) return;
    ({ target, setUpstream } = choice);
  }
  const to = `${target.remote}/${target.branch}`;
  try {
    const result = await runOperation(key, `Pushing ${branch} to ${to}`, (opId) => commands.opPush(key, opId, branch, target, setUpstream));
    const output = `${result.stdout}\n${result.stderr}`;
    if (result.ok) {
      const tracking = setUpstream ? ` ${branch} now tracks it.` : "";
      sessions.notifySuccess(/Everything up-to-date/i.test(output) ? `${to} already has every commit on ${branch}.` : `Pushed ${branch} to ${to}.${tracking}`);
    } else if (result.stderr.includes("[rejected]")) {
      sessions.notify(`${to} has commits that ${branch} does not have. Pull them first, then push again.`, {
        label: "Pull…",
        run: () => openPullDialog(sessions.sessions.get(key) ?? null),
      });
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
  const autostash = await confirmAutostash(session, "Sync");
  if (autostash === null) return;
  const mode = await pullModeFor(key, branch);
  const to = `${upstream.remote}/${upstream.branch}`;
  try {
    const result = await runOperation(key, `Syncing ${branch} with ${to}`, (opId) => commands.opSync(key, opId, branch, mode, autostash));
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
      message: `${pullPart}The push was stopped, so ${to} may not have ${branch}'s new commits yet; sync again to send them.`,
      kind: "error",
      action: null,
      failed: false,
    };
  }
  const push = result.push;
  if (push?.ok) return { message: `${pullPart}Pushed ${branch} to ${to}.`, kind: pulled.kind, action: null, failed: false };
  if (push && push.stderr.includes("[rejected]")) {
    return {
      message: `${pullPart}${to} received new commits while the sync was running, so the push was refused. Sync again.`,
      kind: "error",
      action: { label: "Sync again", run: () => syncCurrentBranch(sessions.sessions.get(key) ?? null) },
      failed: false,
    };
  }
  const detail = push ? networkFailureText(push, `Pushing ${branch} to ${to}`) : `Pushing ${branch} to ${to} did not run.`;
  return { message: `${pullPart}${detail}`, kind: "error", action: null, failed: false };
}

/**
 * Merges `target` (a branch name or an oid) into the checked-out branch; `noFf` makes a merge
 * commit even when a fast-forward would do. A conflicted merge stays open, and the bar above the
 * commits offers Continue and Abort.
 */
export async function mergeInto(key: string, target: string, label: string = target, noFf = false): Promise<void> {
  const sessions = useSessionsStore();
  const into = currentBranch(sessions.sessions.get(key)) ?? "the current branch";
  try {
    const result = await runOp(key, () => commands.opMerge(key, target, noFf));
    const outcome = mergeOutcome(result, label, into);
    if (outcome.kind === "success") sessions.notifySuccess(outcome.message);
    else sessions.notify(outcome.message);
  } catch (e) {
    sessions.notify(errorMessage(e));
  }
}

/** What to report after a merge of `label` into `into` ran; `failed` is a plain failure, with nothing left open. */
export function mergeOutcome(result: OpResult, label: string, into: string): { message: string; kind: "success" | "error"; failed: boolean } {
  const output = `${result.stdout}\n${result.stderr}`;
  if (result.ok) {
    const message = /Already up to date/i.test(output)
      ? `${into} already has every commit on ${label}.`
      : /Fast-forward/.test(output)
        ? `Fast-forwarded ${into} to ${label}.`
        : `Merged ${label} into ${into}.`;
    return { message, kind: "success", failed: false };
  }
  if (result.conflicts) return { message: `Merging ${label} into ${into} stopped on conflicts. ${RESOLVE_HINT}`, kind: "error", failed: false };
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
  return null;
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
    const result = await runOp(key, () => commands.opReset(key, mode, oid));
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
    const result = await runOp(key, () => (pick ? commands.opCherryPick(key, row.oid, mainline) : commands.opRevert(key, row.oid, mainline)));
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
  const verb = OPERATION_VERBS[op.kind];
  try {
    const result = await runOp(key, () => commands.opContinue(key, op.kind));
    const output = `${result.stdout}\n${result.stderr}`;
    if (result.ok) {
      sessions.notifySuccess(
        op.kind === "merge"
          ? `Merged ${names.theirs} into ${names.ours}.`
          : op.kind === "rebase"
            ? `Finished rebasing onto ${names.ours}.`
            : op.kind === "cherryPick"
              ? `Cherry-picked ${names.theirs} onto ${names.ours}.`
              : `Reverted ${op.commit === null ? "the commit" : shortSha(op.commit)} with a new commit on ${names.ours}.`,
      );
    } else if (result.conflicts) {
      sessions.notify(`The rebase went on to the next commit, which conflicts too. ${RESOLVE_HINT}`);
    } else if (/nothing to commit|No changes|is now empty/i.test(output)) {
      const way = op.kind === "rebase" || op.kind === "cherryPick" ? "Skip leaves the commit out." : `Abort the ${verb} to stop it.`;
      sessions.notify(`${opFailureText(result, `Continuing the ${verb}`)} After resolving, nothing is left to commit. ${way}`);
    } else {
      sessions.notify(opFailureText(result, `Continuing the ${verb}`));
    }
  } catch (e) {
    sessions.notify(errorMessage(e));
  }
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
  return [...local, ...remote].map((r) => ({ short: r.short, remote: r.kind.type === "remoteBranch", target: r.target }));
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
  const blocked = dirtyTreeReason(session);
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
  await useDialogsStore().open("merge", { key: session.key, into, branches: otherBranches(session) });
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
  await useDialogsStore().open("rebase", { key: session.key, branch, branches: otherBranches(session), upstream: upstream?.replace(/^refs\/remotes\//, "") ?? null });
}

/** What to report after `git rebase <onto>` ran; `failed` is a plain failure, with nothing left open. */
export function rebaseOutcome(result: OpResult, branch: string, onto: string): { message: string; kind: "success" | "error"; failed: boolean } {
  const output = `${result.stdout}\n${result.stderr}`;
  if (result.ok) {
    const message = /is up to date/i.test(output) ? `${branch} already has every commit on ${onto}.` : `Rebased ${branch} onto ${onto}.`;
    return { message, kind: "success", failed: false };
  }
  if (result.conflicts) return { message: `Rebasing ${branch} onto ${onto} stopped on conflicts. ${RESOLVE_HINT}`, kind: "error", failed: false };
  return { message: opFailureText(result, `Rebasing ${branch} onto ${onto}`), kind: "error", failed: true };
}

/** Opens the Rename Branch dialog for local branch `name`. The dialog runs the command itself. */
export async function renameBranch(key: string, name: string): Promise<void> {
  await useDialogsStore().open("renameBranch", { key, name });
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
    `Applied ${name}. The stash is kept.`,
    `Applied ${name} with conflicts. Resolve them in the working tree; the stash is kept.`,
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
    message: `${label}\n\nThe stash is deleted without being applied, and its changes are lost.`,
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
function filesLabel(paths: string[]): string {
  return paths.length === 1 ? (paths[0] ?? "1 file") : `${paths.length} files`;
}

/** Why the selected files cannot be staged, unstaged or discarded, or null when they can. */
export function fileActionReason(session: RepoSession | null | undefined, action: "stage" | "unstage" | "discard"): string | null {
  if (!session) return "Open a repository first.";
  if (session.info.isBare) return "A bare repository has no working tree.";
  if (session.selectedRow?.kind !== "workingTree") return "Select the Working Tree row, then the files.";
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
    sessions.notify(reason ?? "Open a repository first.");
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
    sessions.notify(reason ?? "Open a repository first.");
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
  try {
    if (p.discardToHead.length > 0) {
      const r = await runOp(key, () => commands.opDiscard(key, p.discardToHead, [], true));
      if (!r.ok) {
        sessions.notify(opFailureText(r, what));
        return;
      }
    }
    if (p.discardToIndex.length > 0 || p.discardUntracked.length > 0) {
      const r = await runOp(key, () => commands.opDiscard(key, p.discardToIndex, p.discardUntracked, false));
      if (!r.ok) {
        sessions.notify(opFailureText(r, what));
        return;
      }
    }
    sessions.notifySuccess(`Discarded the changes to ${filesLabel(p.discardFiles)}.`);
  } catch (e) {
    sessions.notify(errorMessage(e));
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
