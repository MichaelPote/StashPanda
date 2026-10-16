<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { LoaderCircleIcon } from "@lucide/vue";
import { commands, type PullMode } from "@/lib/bindings";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Label } from "@/components/ui/label";
import { errorMessage, IpcError } from "@/lib/ipc";
import { pullOutcome, runOperation, stopOperation } from "@/lib/repoActions";
import { cn } from "@/lib/utils";
import { useDialogsStore } from "@/stores/dialogs";
import { useSessionsStore } from "@/stores/sessions";
import { useUiStateStore } from "@/stores/uiState";

const dialogs = useDialogsStore();
const sessions = useSessionsStore();
const uiState = useUiStateStore();

const request = computed(() => dialogs.current("pull"));

const sourceShort = ref("");
const mode = ref<PullMode>("merge");
const error = ref<string | null>(null);
/** True while this dialog is showing a pull it started. */
const running = ref(false);

watch(request, (r) => {
  if (!r) return;
  sourceShort.value = r.source.short;
  mode.value = r.mode;
  error.value = null;
  running.value = false;
});

/** The pull's progress, which the status bar shows too. */
const operation = computed(() => (request.value && running.value ? (sessions.sessions.get(request.value.key)?.operation ?? null) : null));
const progressLine = computed(() => operation.value?.line ?? "");
/** False once the fetch is done and the merge or rebase step, which cannot be stopped, has begun. */
const stoppable = computed(() => operation.value?.cancellable ?? true);
const stopping = computed(() => operation.value?.stopping ?? false);

const source = computed(() => request.value?.sources.find((s) => s.short === sourceShort.value) ?? null);
const canSubmit = computed(() => request.value !== null && source.value !== null && !running.value);
/** The percentage in the latest progress update, for the bar; null when it has none. */
const percent = computed(() => {
  const m = /(\d{1,3})%/.exec(progressLine.value);
  return m ? Math.min(100, Number(m[1])) : null;
});

const MODES: { id: PullMode; label: string; detail: (into: string) => string }[] = [
  { id: "merge", label: "Merge", detail: (into) => `Fast-forward if ${into} has no commits of its own; otherwise make a merge commit.` },
  { id: "rebase", label: "Rebase", detail: (into) => `Replay ${into}'s own commits on top of the remote branch, so the history stays in one line.` },
  {
    id: "fastForwardOnly",
    label: "Fast-forward only",
    detail: (into) => `Only move ${into} forward. Git refuses if both sides have new commits.`,
  },
];

const selectClass =
  "well h-8 w-full rounded-lg border px-2.5 text-base outline-none focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/50";

/** Closing the dialog while the pull runs leaves it running in the status bar; its outcome then shows as a notice. */
function onOpenChange(open: boolean): void {
  if (!open) dialogs.cancel();
}

function stop(): void {
  if (request.value) void stopOperation(request.value.key);
}

async function submit(): Promise<void> {
  const r = request.value;
  const from = source.value;
  if (!r || !from || !canSubmit.value) return;
  const chosen = mode.value;
  uiState.patchDebounced({ perRepo: { [r.key]: { pullMode: chosen } } });
  running.value = true;
  error.value = null;
  /** True while this dialog is still showing this pull, so the outcome can go inline. */
  const showing = () => request.value === r;
  try {
    const result = await runOperation(r.key, `Pulling ${from.short} into ${r.into}`, (opId) =>
      commands.opPull(r.key, opId, from.remote, from.branch, r.into, chosen, r.autostash),
    );
    const outcome = pullOutcome(result, from.short, r.into, chosen);
    if (outcome.failed && showing()) {
      error.value = outcome.message;
      return;
    }
    if (outcome.kind === "success") sessions.notifySuccess(outcome.message);
    else sessions.notify(outcome.message, outcome.action);
    if (showing()) dialogs.resolve("pull", result);
  } catch (e) {
    const cancelled = e instanceof IpcError && e.error.kind === "cancelled";
    const message = cancelled ? `The pull was stopped while fetching. Nothing was merged into ${r.into}.` : errorMessage(e);
    if (showing()) error.value = message;
    else sessions.notify(message);
  } finally {
    if (showing()) running.value = false;
  }
}
</script>

<template>
  <Dialog :open="request !== null" @update:open="onOpenChange">
    <DialogContent v-if="request" class="sm:max-w-xl" size-key="pull">
      <DialogHeader>
        <DialogTitle>Pull</DialogTitle>
        <DialogDescription>
          Fetches the remote branch and brings its new commits into
          <span class="font-mono">{{ request.into }}</span>.
        </DialogDescription>
      </DialogHeader>

      <form class="grid gap-3" @submit.prevent="submit">
        <div class="grid gap-1.5">
          <Label for="pull-source" class="text-base">Remote branch</Label>
          <select id="pull-source" v-model="sourceShort" :class="selectClass" :disabled="running">
            <option v-for="s in request.sources" :key="s.short" :value="s.short">
              {{ s.short }}{{ request.sourceIsUpstream && s.short === request.source.short ? " (tracked by " + request.into + ")" : "" }}
            </option>
          </select>
        </div>

        <div role="radiogroup" aria-label="How to bring in the commits" class="grid gap-1">
          <label
            v-for="m in MODES"
            :key="m.id"
            :class="cn('flex items-start gap-2 rounded-md px-2 py-1.5 select-none', mode === m.id ? 'row-selected' : 'hover:bg-row-hover', running && 'opacity-60')"
          >
            <input v-model="mode" type="radio" name="pull-mode" class="mt-1 size-3.5 shrink-0 accent-primary" :value="m.id" :disabled="running" />
            <span class="grid gap-0.5">
              <span class="text-base">{{ m.label }}</span>
              <span class="text-xs text-muted-foreground">{{ m.detail(request.into) }}</span>
            </span>
          </label>
        </div>

        <p v-if="request.autostash" class="text-xs text-muted-foreground">
          Your uncommitted changes are stashed first and put back after the pull.
        </p>

        <div v-if="running" class="grid gap-1.5" role="status" aria-live="polite">
          <div class="h-1.5 overflow-hidden rounded-full bg-foreground/10">
            <div v-if="percent !== null" class="h-full bg-primary transition-[width] duration-150" :style="{ width: `${percent}%` }" />
            <div v-else class="h-full w-1/3 animate-pulse bg-primary/60" />
          </div>
          <p class="truncate font-mono text-xs text-muted-foreground" :title="progressLine">
            {{ stopping ? "Stopping…" : progressLine || "Starting…" }}
          </p>
        </div>

        <p v-if="error" class="rounded-md border border-destructive/40 bg-destructive/10 px-2.5 py-1.5 text-base break-words text-destructive">
          {{ error }}
        </p>
      </form>

      <DialogFooter>
        <Button
          v-if="running"
          type="button"
          variant="outline"
          :disabled="stopping || !stoppable"
          :title="stoppable ? 'Stop fetching. Nothing is merged.' : 'The fetch is done; the merge or rebase step cannot be stopped.'"
          @click="stop"
        >
          Stop
        </Button>
        <Button v-else type="button" variant="outline" @click="dialogs.cancel()">Cancel</Button>
        <Button type="button" :disabled="!canSubmit" @click="submit">
          <LoaderCircleIcon v-if="running" class="animate-spin" />
          {{ running ? "Pulling…" : "Pull" }}
        </Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
</template>
