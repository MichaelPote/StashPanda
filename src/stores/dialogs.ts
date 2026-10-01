import { ref, shallowRef } from "vue";
import { defineStore } from "pinia";
import type { OpResult, PullMode, PushTarget, StashEntry } from "@/lib/bindings";
import type { CommitFile } from "@/lib/files";

export interface CreateBranchPayload {
  key: string;
  /** Revision the new branch starts from (an oid, or `HEAD`). */
  startPoint: string;
  /** How the dialog names the start point: a branch name or a short sha. */
  startLabel: string;
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

/** Every dialog the store can open, with what the caller passes in and what the promise resolves to. */
export interface DialogSpecs {
  createBranch: { payload: CreateBranchPayload; result: OpResult | null };
  createTag: { payload: CreateTagPayload; result: OpResult | null };
  commit: { payload: CommitPayload; result: OpResult | null };
  stashPush: { payload: StashPushPayload; result: OpResult | null };
  stashApply: { payload: StashApplyPayload; result: StashApplyChoice | null };
  pull: { payload: PullPayload; result: OpResult | null };
  push: { payload: PushPayload; result: PushChoice | null };
  confirm: { payload: ConfirmPayload; result: boolean };
  /** Resolves true when the changes were saved. */
  settings: { payload: SettingsPayload; result: boolean };
  about: { payload: null; result: null };
}

export type DialogKind = keyof DialogSpecs;

/** What each dialog resolves with when it is dismissed without a decision. */
const CANCEL_RESULTS: { [K in DialogKind]: DialogSpecs[K]["result"] } = {
  createBranch: null,
  createTag: null,
  commit: null,
  stashPush: null,
  stashApply: null,
  pull: null,
  push: null,
  confirm: false,
  settings: false,
  about: null,
};

interface ActiveDialog<K extends DialogKind = DialogKind> {
  kind: K;
  payload: DialogSpecs[K]["payload"];
  resolve: (result: DialogSpecs[K]["result"]) => void;
}

/**
 * One dialog at a time, opened from anywhere (the native menu bar, a context menu, a panel) and
 * rendered once in `App.vue`. `open()` resolves when the dialog closes.
 */
export const useDialogsStore = defineStore("dialogs", () => {
  const active = shallowRef<ActiveDialog | null>(null);
  /** The Add Repository dialog keeps its own flag: it has no result and several openers. */
  const addRepoOpen = ref(false);
  /** Set while the same dialog edits an existing library entry rather than adding one. */
  const editRepoKey = ref<string | null>(null);

  function editRepo(key: string): void {
    editRepoKey.value = key;
    addRepoOpen.value = true;
  }

  function open<K extends DialogKind>(kind: K, payload: DialogSpecs[K]["payload"]): Promise<DialogSpecs[K]["result"]> {
    // A dialog still open when the next one is requested counts as cancelled.
    cancel();
    return new Promise<DialogSpecs[K]["result"]>((resolve) => {
      active.value = { kind, payload, resolve } as ActiveDialog;
    });
  }

  /** The payload of the open dialog when it is of `kind`, else null. Dialog components watch this. */
  function current<K extends DialogKind>(kind: K): DialogSpecs[K]["payload"] | null {
    const a = active.value;
    return a && a.kind === kind ? (a.payload as DialogSpecs[K]["payload"]) : null;
  }

  /** Closes the open dialog of `kind` with a result. Ignored when another dialog is open. */
  function resolve<K extends DialogKind>(kind: K, result: DialogSpecs[K]["result"]): void {
    const a = active.value;
    if (!a || a.kind !== kind) return;
    active.value = null;
    (a.resolve as (r: DialogSpecs[K]["result"]) => void)(result);
  }

  /** Closes whatever dialog is open as if the user had pressed Cancel or Escape. */
  function cancel(): void {
    const a = active.value;
    if (!a) return;
    active.value = null;
    (a.resolve as (r: unknown) => void)(CANCEL_RESULTS[a.kind]);
  }

  return { active, addRepoOpen, editRepoKey, editRepo, open, current, resolve, cancel };
});
