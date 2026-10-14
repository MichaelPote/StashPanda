import { ref, shallowRef } from "vue";
import { defineStore } from "pinia";
import type { OpResult, PassphraseMemory, PullMode, PushTarget, StashEntry } from "@/lib/bindings";
import type { CommitFile } from "@/lib/files";

export interface CreateBranchPayload {
  key: string;
  /** Revision the new branch starts from (an oid, or `HEAD`). */
  startPoint: string;
  /** How the dialog names the start point: a branch name or a short sha. */
  startLabel: string;
}

export interface RenameBranchPayload {
  key: string;
  /** The local branch to rename. */
  name: string;
}

export interface CreateTagPayload {
  key: string;
  target: string;
  targetLabel: string;
}

export interface CommitPayload {
  key: string;
  /** Every outstanding file in the working tree. */
  files: CommitFile[];
  /** Paths ticked when the dialog opens. */
  ticked: string[];
  /** The ticks came from the file list selection rather than the preselect setting. */
  fromSelection: boolean;
  /** Checked-out branch, or null when HEAD is detached. */
  branch: string | null;
  /** False on an unborn branch, where there is no commit to amend. */
  hasHead: boolean;
  /**
   * A merge, cherry-pick or revert is in progress, and this commit finishes it: with every staged
   * change (Git refuses a partial commit then), and `message` as Git prepared it in MERGE_MSG.
   */
  finishing?: { what: string; message: string | null };
}

export interface StashPushPayload {
  key: string;
  /** Checked-out branch, or null when HEAD is detached. */
  branch: string | null;
  /** Staged and unstaged tracked files; a plain `git stash push` saves only these. */
  trackedCount: number;
  untrackedCount: number;
}

export interface StashApplyPayload {
  key: string;
}

/** The stash picked in the Apply Stash dialog, and whether to drop it afterwards (a pop). */
export interface StashApplyChoice {
  stash: StashEntry;
  drop: boolean;
}

/** A remote branch the Pull dialog can pull from. */
export interface PullSource {
  /** `origin/dev` */
  short: string;
  remote: string;
  /** The branch's name on the remote: `dev`. */
  branch: string;
}

export interface PullPayload {
  key: string;
  /** The checked-out branch the commits are pulled into. */
  into: string;
  /** Every remote branch, in the order the refs list gives them. */
  sources: PullSource[];
  /** The remote branch picked when the dialog opens: the branch's upstream when it has one. */
  source: PullSource;
  /** `source` is the branch's upstream. */
  sourceIsUpstream: boolean;
  /** The mode picked when the dialog opens. */
  mode: PullMode;
  /** The working tree has uncommitted changes and the user chose to pull anyway with `--autostash`. */
  autostash: boolean;
}

/** A branch the Merge and Rebase dialogs offer. */
export interface BranchChoice {
  /** `dev`, or `origin/dev` for a remote branch. */
  short: string;
  remote: boolean;
  /** The commit it points at. */
  target: string;
}

export interface MergePayload {
  key: string;
  /** The checked-out branch the chosen branch is merged into. */
  into: string;
  /** Local branches other than `into`, then remote branches. */
  branches: BranchChoice[];
}

export interface RebasePayload {
  key: string;
  /** The checked-out branch whose own commits are replayed. */
  branch: string;
  /** Local branches other than `branch`, then remote branches. */
  branches: BranchChoice[];
  /** The remote branch `branch` tracks (`origin/main`), picked first; null when it tracks none. */
  upstream: string | null;
}

export interface PushPayload {
  key: string;
  /** The checked-out branch, which tracks no remote branch yet. */
  branch: string;
  /** Remote names, in config order. */
  remotes: string[];
}

/** Where the Push dialog chose to push, and whether the branch should track it from now on. */
export interface PushChoice {
  target: PushTarget;
  setUpstream: boolean;
}

export interface SettingsPayload {
  /** The page to show first (a section `id` from `components/settings/sections.ts`); General when absent. */
  section?: string;
}

export interface ConfirmPayload {
  title: string;
  message: string;
  confirmLabel?: string;
  destructive?: boolean;
}

export interface PassphrasePayload {
  /**
   * The key file. The dialog checks the passphrase against it with `keysUnlock` and remembers it
   * as chosen. Null for key text that is not in a file yet (an import): the caller checks it.
   */
  path: string | null;
  /** How the dialog names the key: its file name. */
  keyName: string;
  /** What the passphrase is for, finishing "StashPanda needs the passphrase …": "to connect to web1". */
  purpose: string;
  /** The last passphrase given was wrong. */
  wrong?: boolean;
  /**
   * Opening a connection: the passphrase must be remembered at least until StashPanda closes,
   * because the connection reconnects by itself, so "only this time" is not offered.
   */
  forConnection?: boolean;
}

/** The passphrase given, and where `keysUnlock` remembered it. */
export interface PassphraseResult {
  passphrase: string;
  remembered: PassphraseMemory;
}

export interface SshHostKeyPayload {
  host: string;
  port: number;
  fingerprint: string;
  /** The key differs from the one in known_hosts: a warning with no way to accept it. */
  changed: boolean;
  /** What was being opened, such as a repository's name. */
  subject: string;
}

export interface SshPasswordPayload {
  host: string;
  port: number;
  user: string;
  /** The last password given was refused. */
  rejected: boolean;
  subject: string;
}

export interface FileHistoryPayload {
  key: string;
  /** The file's path at `rev`. Older commits may know it by another name. */
  path: string;
  /** The commit the history starts from; null for HEAD. */
  rev: string | null;
}

export interface BlamePayload {
  key: string;
  /** The file's path at `rev`. */
  path: string;
  /** The commit whose version is blamed; null for the working tree version. */
  rev: string | null;
  /** A line (counting from 1) to scroll into view once the lines have loaded. */
  line?: number;
}

/** The Add Repository dialog's tabs: add a folder that is a repository already, or clone one. */
export type AddRepoTab = "existing" | "clone";

/** Every dialog the store can open, with what the caller passes in and what the promise resolves to. */
export interface DialogSpecs {
  createBranch: { payload: CreateBranchPayload; result: OpResult | null };
  createTag: { payload: CreateTagPayload; result: OpResult | null };
  renameBranch: { payload: RenameBranchPayload; result: OpResult | null };
  commit: { payload: CommitPayload; result: OpResult | null };
  stashPush: { payload: StashPushPayload; result: OpResult | null };
  stashApply: { payload: StashApplyPayload; result: StashApplyChoice | null };
  pull: { payload: PullPayload; result: OpResult | null };
  merge: { payload: MergePayload; result: OpResult | null };
  rebase: { payload: RebasePayload; result: OpResult | null };
  push: { payload: PushPayload; result: PushChoice | null };
  confirm: { payload: ConfirmPayload; result: boolean };
  fileHistory: { payload: FileHistoryPayload; result: null };
  blame: { payload: BlamePayload; result: null };
  passphrase: { payload: PassphrasePayload; result: PassphraseResult | null };
  /** Resolves true once the host key has been recorded in known_hosts. */
  sshHostKey: { payload: SshHostKeyPayload; result: boolean };
  /** Resolves true once the password has been handed to `sshSetPassword`. */
  sshPassword: { payload: SshPasswordPayload; result: boolean };
  /** Resolves true when the changes were saved. */
  settings: { payload: SettingsPayload; result: boolean };
  about: { payload: null; result: null };
}

export type DialogKind = keyof DialogSpecs;

/** What each dialog resolves with when it is dismissed without a decision. */
const CANCEL_RESULTS: { [K in DialogKind]: DialogSpecs[K]["result"] } = {
  createBranch: null,
  createTag: null,
  renameBranch: null,
  commit: null,
  stashPush: null,
  stashApply: null,
  pull: null,
  merge: null,
  rebase: null,
  push: null,
  confirm: false,
  fileHistory: null,
  blame: null,
  passphrase: null,
  sshHostKey: false,
  sshPassword: false,
  settings: false,
  about: null,
};

/**
 * Dialogs that open on top of another open dialog rather than replacing it: a confirmation, and
 * the SSH prompts, which the SSH Keys page in Settings and an open started from a dialog need.
 */
const STACKED: ReadonlySet<DialogKind> = new Set<DialogKind>(["confirm", "passphrase", "sshHostKey", "sshPassword"]);

interface ActiveDialog<K extends DialogKind = DialogKind> {
  kind: K;
  payload: DialogSpecs[K]["payload"];
  resolve: (result: DialogSpecs[K]["result"]) => void;
}

/**
 * One dialog at a time, opened from anywhere (the native menu bar, a context menu, a panel) and
 * rendered once in `App.vue`. `open()` resolves when the dialog closes. The exception is a
 * `STACKED` dialog asked for while another dialog is open (a stale lock found while committing,
 * a passphrase asked for by the SSH Keys page): it opens on top, and the dialog behind it stays
 * open.
 */
export const useDialogsStore = defineStore("dialogs", () => {
  const active = shallowRef<ActiveDialog | null>(null);
  /** A stacked dialog shown on top of `active`. */
  const overlay = shallowRef<ActiveDialog | null>(null);
  /** The Add Repository dialog keeps its own flag: it has no result and several openers. */
  const addRepoOpen = ref(false);
  /** Set while the same dialog edits an existing library entry rather than adding one. */
  const editRepoKey = ref<string | null>(null);
  /** The tab the Add Repository dialog opens on. The dialog puts it back to "existing" as it closes. */
  const addRepoTab = ref<AddRepoTab>("existing");

  /**
   * Opens the Add Repository dialog, on `tab`. Does nothing while another dialog is open: it would
   * open underneath that one, since it comes first in the page, yet as the newest modal dialog it
   * would take every click, so the dialog in view could not be used. (A native menu item, such as
   * File > Open Repository, still works while a dialog is open.) Returns whether it opened.
   */
  function showAddRepo(tab: AddRepoTab = "existing"): boolean {
    if (active.value !== null || overlay.value !== null) return false;
    addRepoTab.value = tab;
    addRepoOpen.value = true;
    return true;
  }

  function editRepo(key: string): void {
    if (active.value !== null || overlay.value !== null) return;
    editRepoKey.value = key;
    showAddRepo();
  }

  /** Opens the Add Repository dialog on its Clone From tab. */
  function cloneRepo(): void {
    showAddRepo("clone");
  }

  function open<K extends DialogKind>(kind: K, payload: DialogSpecs[K]["payload"]): Promise<DialogSpecs[K]["result"]> {
    if (STACKED.has(kind) && active.value !== null && !STACKED.has(active.value.kind)) {
      closeOverlay();
      return new Promise<DialogSpecs[K]["result"]>((resolve) => {
        overlay.value = { kind, payload, resolve } as ActiveDialog;
      });
    }
    // A dialog still open when the next one is requested counts as cancelled.
    cancel();
    return new Promise<DialogSpecs[K]["result"]>((resolve) => {
      active.value = { kind, payload, resolve } as ActiveDialog;
    });
  }

  /** Closes the stacked dialog with `result`, or as cancelled when no result is given. */
  function closeOverlay(result?: unknown): boolean {
    const o = overlay.value;
    if (!o) return false;
    overlay.value = null;
    (o.resolve as (r: unknown) => void)(result === undefined ? CANCEL_RESULTS[o.kind] : result);
    return true;
  }

  /** The payload of the open dialog when it is of `kind`, else null. Dialog components watch this. */
  function current<K extends DialogKind>(kind: K): DialogSpecs[K]["payload"] | null {
    const o = overlay.value;
    if (o && o.kind === kind) return o.payload as DialogSpecs[K]["payload"];
    const a = active.value;
    return a && a.kind === kind ? (a.payload as DialogSpecs[K]["payload"]) : null;
  }

  /** Closes the open dialog of `kind` with a result. Ignored when another dialog is open. */
  function resolve<K extends DialogKind>(kind: K, result: DialogSpecs[K]["result"]): void {
    if (overlay.value?.kind === kind && closeOverlay(result)) return;
    const a = active.value;
    if (!a || a.kind !== kind) return;
    active.value = null;
    (a.resolve as (r: DialogSpecs[K]["result"]) => void)(result);
  }

  /** Closes whatever dialog is open, and a stacked dialog on top of it, as if the user had pressed Cancel or Escape. */
  function cancel(): void {
    closeOverlay();
    const a = active.value;
    if (!a) return;
    active.value = null;
    (a.resolve as (r: unknown) => void)(CANCEL_RESULTS[a.kind]);
  }

  return { active, overlay, addRepoOpen, editRepoKey, addRepoTab, showAddRepo, editRepo, cloneRepo, open, current, resolve, cancel };
});
