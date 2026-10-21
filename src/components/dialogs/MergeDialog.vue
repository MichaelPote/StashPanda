<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { LoaderCircleIcon } from "@lucide/vue";
import { commands } from "@/lib/bindings";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Label } from "@/components/ui/label";
import { errorMessage } from "@/lib/ipc";
import { mergeOutcome, runOp } from "@/lib/repoActions";
import { useDialogsStore } from "@/stores/dialogs";
import { useSessionsStore } from "@/stores/sessions";

const dialogs = useDialogsStore();
const sessions = useSessionsStore();

const request = computed(() => dialogs.current("merge"));

const chosen = ref("");
const noFf = ref(false);
const error = ref<string | null>(null);
const submitting = ref(false);

watch(request, (r) => {
  if (!r) return;
  chosen.value = r.branches[0]?.short ?? "";
  noFf.value = false;
  error.value = null;
  submitting.value = false;
});

const local = computed(() => request.value?.branches.filter((b) => !b.remote) ?? []);
const remote = computed(() => request.value?.branches.filter((b) => b.remote) ?? []);
const canSubmit = computed(() => chosen.value !== "" && !submitting.value);

const selectClass =
  "well h-8 w-full rounded-lg border px-2.5 text-base outline-none focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/50";

function onOpenChange(open: boolean): void {
  if (!open) dialogs.cancel();
}

async function submit(): Promise<void> {
  const r = request.value;
  if (!r || !canSubmit.value) return;
  const from = chosen.value;
  submitting.value = true;
  error.value = null;
  try {
    const revision = r.branches.find((b) => b.short === from)?.revision ?? from;
    const result = await runOp(r.key, () => commands.opMerge(r.key, revision, noFf.value, r.into, r.autostash));
    const outcome = mergeOutcome(result, from, r.into, r.autostash);
    // A plain failure leaves nothing open, so the dialog stays for another choice.
    if (outcome.failed) {
      error.value = outcome.message;
      return;
    }
    if (outcome.kind === "success") sessions.notifySuccess(outcome.message);
    else sessions.notify(outcome.message);
    dialogs.resolve("merge", result);
  } catch (e) {
    error.value = errorMessage(e);
  } finally {
    submitting.value = false;
  }
}
</script>

<template>
  <Dialog :open="request !== null" @update:open="onOpenChange">
    <DialogContent v-if="request" class="sm:max-w-lg" size-key="merge">
      <DialogHeader>
        <DialogTitle>Merge into {{ request.into }}</DialogTitle>
        <DialogDescription>
          Brings the commits of the branch you choose into <span class="font-mono">{{ request.into }}</span>. If both have changed the same
          lines, the merge stops so you can resolve the conflicts.
        </DialogDescription>
      </DialogHeader>

      <form class="grid gap-3" @submit.prevent="submit">
        <div class="grid gap-1.5">
          <Label for="merge-branch" class="text-base">Branch to merge</Label>
          <select id="merge-branch" v-model="chosen" :class="selectClass" :disabled="submitting">
            <optgroup v-if="local.length > 0" label="Local branches">
              <option v-for="b in local" :key="b.short" :value="b.short">{{ b.short }}</option>
            </optgroup>
            <optgroup v-if="remote.length > 0" label="Remote branches">
              <option v-for="b in remote" :key="b.short" :value="b.short">{{ b.short }}</option>
            </optgroup>
          </select>
        </div>

        <div class="grid gap-1">
          <label class="flex items-center gap-2 text-base select-none">
            <input v-model="noFf" type="checkbox" class="size-3.5 accent-primary" :disabled="submitting" />
            Create a merge commit even when a fast-forward is possible
          </label>
          <p class="text-xs text-muted-foreground">
            <template v-if="noFf">The merge always makes a merge commit, so the history shows where the branch was merged (git merge --no-ff).</template>
            <template v-else>When {{ request.into }} has no commits of its own, it simply moves forward to the branch, with no merge commit.</template>
          </p>
        </div>

        <p v-if="request.autostash" class="text-xs text-muted-foreground">
          Your uncommitted changes are stashed first and put back after the merge.
        </p>

        <p v-if="error" class="rounded-md border border-destructive/40 bg-destructive/10 px-2.5 py-1.5 text-base break-words text-destructive">
          {{ error }}
        </p>
      </form>

      <DialogFooter>
        <Button type="button" variant="outline" @click="dialogs.cancel()">Cancel</Button>
        <Button type="button" :disabled="!canSubmit" @click="submit">
          <LoaderCircleIcon v-if="submitting" class="animate-spin" />
          Merge
        </Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
</template>
