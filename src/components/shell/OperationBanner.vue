<script setup lang="ts">
import { computed, ref } from "vue";
import { CherryIcon, GitMergeIcon, LoaderCircleIcon, RotateCcwIcon, Undo2Icon } from "@lucide/vue";
import { Tip } from "@/components/ui/tooltip";
import { OPERATION_VERBS, operationNames } from "@/lib/conflicts";
import { abortOperation, continueOperation, continueReason, skipOperation } from "@/lib/repoActions";
import { useDialogsStore } from "@/stores/dialogs";
import type { RepoSession } from "@/stores/sessions";

/**
 * The bar across the top of a repository while a merge, rebase, cherry-pick or revert has stopped
 * part way: what it is, how many files are still conflicted, and Continue, Skip and Abort.
 */
const props = defineProps<{
  session: RepoSession;
}>();

const dialogs = useDialogsStore();

const op = computed(() => props.session.status?.inProgress ?? null);
const names = computed(() => operationNames(props.session.status, props.session.refs, props.session.stashConflict));
const conflicted = computed(() => props.session.status?.conflictedCount ?? 0);
const verb = computed(() => (op.value ? OPERATION_VERBS[op.value.kind] : ""));
/** Not for an interrupted rebase: skipping would drop the commit Git was part way through, which only Abort keeps. */
const canSkip = computed(() => (op.value?.kind === "rebase" && !op.value.interrupted) || op.value?.kind === "cherryPick");

const ICONS = { merge: GitMergeIcon, rebase: RotateCcwIcon, cherryPick: CherryIcon, revert: Undo2Icon } as const;

const stateText = computed(() => {
  if (op.value?.interrupted) return "Git was stopped part way through a commit, so it cannot go on safely: Abort puts the branch back as it was.";
  const n = conflicted.value;
  if (n > 0) return `${n === 1 ? "1 file is" : `${n} files are`} still conflicted.`;
  return op.value?.kind === "rebase" ? "No conflicts left: Continue goes on with the rebase." : `No conflicts left: Continue finishes the ${verb.value}.`;
});

/** Which button is running, so the bar shows a spinner on it and ignores further clicks. */
const busy = ref<"continue" | "skip" | "abort" | null>(null);
/** Skip and Abort ask first; no spinner while the question is open. */
const asking = computed(() => dialogs.active !== null);

async function run(which: "continue" | "skip" | "abort"): Promise<void> {
  if (busy.value !== null) return;
  if (which === "continue" && continueReason(props.session)) return;
  busy.value = which;
  try {
    if (which === "continue") await continueOperation(props.session);
    else if (which === "skip") await skipOperation(props.session);
    else await abortOperation(props.session);
  } finally {
    busy.value = null;
  }
}

const continueBlocked = computed(() => continueReason(props.session));
const continueTitle = computed(() => {
  if (continueBlocked.value) return `Continue: ${continueBlocked.value}`;
  return op.value?.kind === "rebase" ? "Commit the resolved changes and go on with the next commit" : `Commit the resolved changes and finish the ${verb.value}`;
});

/** Continue and Skip in the conflict yellow; Abort in red, since it throws the work away. */
const BUTTON_CLASS = "bar-key key-conflict";
const ABORT_CLASS = "bar-key key-danger";
</script>

<template>
  <div v-if="op" class="flex min-h-[36px] shrink-0 items-center gap-2 border-b border-border band-conflict px-2 py-1 text-base" role="status" data-operation-banner>
    <component :is="ICONS[op.kind]" class="size-3.5 shrink-0 text-(--state-conflict)" aria-hidden="true" />
    <span class="shrink-0 font-medium">{{ names.title }}</span>
    <span v-if="op.subject" class="min-w-0 truncate text-muted-foreground" :title="op.subject">{{ op.subject }}</span>
    <span :class="['shrink-0 text-xs', conflicted > 0 ? 'text-foreground' : 'text-muted-foreground']">{{ stateText }}</span>
    <span class="ml-auto flex shrink-0 items-center gap-3 pb-0.5">
      <!-- aria-disabled rather than disabled, so the tooltip with the reason still shows. -->
      <Tip :content="continueTitle">
        <button
          type="button"
          :class="BUTTON_CLASS"
          :aria-disabled="continueBlocked !== null || busy !== null"
          data-operation-continue
          @click="run('continue')"
        >
          <LoaderCircleIcon v-if="busy === 'continue'" class="size-3 animate-spin" />
          Continue
        </button>
      </Tip>
      <Tip v-if="canSkip" :content="`Leave out ${names.theirs} and throw away its changes${op.kind === 'rebase' ? ', then go on with the next commit' : ''}`">
        <button
          type="button"
          :class="BUTTON_CLASS"
          :aria-disabled="busy !== null"
          data-operation-skip
          @click="run('skip')"
        >
          <LoaderCircleIcon v-if="busy === 'skip' && !asking" class="size-3 animate-spin" />
          Skip
        </button>
      </Tip>
      <Tip :content="`Stop the ${verb} and put the branch back where it was`">
        <button
          type="button"
          :class="ABORT_CLASS"
          :aria-disabled="busy !== null"
          data-operation-abort
          @click="run('abort')"
        >
          <LoaderCircleIcon v-if="busy === 'abort' && !asking" class="size-3 animate-spin" />
          Abort
        </button>
      </Tip>
    </span>
  </div>
</template>
