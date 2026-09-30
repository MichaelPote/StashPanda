<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { LoaderCircleIcon } from "@lucide/vue";
import { commands } from "@/lib/bindings";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { errorMessage } from "@/lib/ipc";
import { opFailureText, runOp } from "@/lib/repoActions";
import { useDialogsStore } from "@/stores/dialogs";
import { useSessionsStore } from "@/stores/sessions";

const dialogs = useDialogsStore();
const sessions = useSessionsStore();

const request = computed(() => dialogs.current("stashPush"));

const message = ref("");
const includeUntracked = ref(false);
const error = ref<string | null>(null);
const submitting = ref(false);

watch(request, (r) => {
  if (!r) return;
  message.value = "";
  // With only untracked files, a plain stash has nothing to save, so they are included from the start.
  includeUntracked.value = r.trackedCount === 0;
  error.value = null;
  submitting.value = false;
});

/** Only untracked files changed, and they are left out: Git would report nothing to save. */
const nothingToSave = computed(() => request.value !== null && request.value.trackedCount === 0 && !includeUntracked.value);
const canSubmit = computed(() => request.value !== null && !submitting.value && !nothingToSave.value);

function filesText(n: number): string {
  return `${n} ${n === 1 ? "file" : "files"}`;
}

function onOpenChange(open: boolean): void {
  if (!open) dialogs.cancel();
}

async function submit(): Promise<void> {
  const r = request.value;
  if (!r || !canSubmit.value) return;
  const text = message.value.trim();
  submitting.value = true;
  error.value = null;
  try {
    const result = await runOp(r.key, commands.opStashPush(r.key, text.length > 0 ? text : null, includeUntracked.value));
    if (!result.ok) {
      error.value = opFailureText(result, "Stashing the changes");
      return;
    }
    // Git exits 0 when it had nothing to save; the status the dialog opened with may be stale.
    if (/no local changes to save/i.test(result.stdout + result.stderr)) {
      error.value = "There were no changes to stash.";
      return;
    }
    sessions.notifySuccess("Stashed the changes as stash@{0}. The working tree is back at the last commit.");
    dialogs.resolve("stashPush", result);
  } catch (e) {
    error.value = errorMessage(e);
  } finally {
    submitting.value = false;
  }
}
</script>

<template>
  <Dialog :open="request !== null" @update:open="onOpenChange">
    <DialogContent v-if="request" class="sm:max-w-lg" size-key="stash">
      <DialogHeader>
        <DialogTitle>Stash changes</DialogTitle>
        <DialogDescription>
          Saves the changes<template v-if="request.branch"> on <span class="font-mono">{{ request.branch }}</span></template>
          as a new stash, and resets the working tree to the last commit.
        </DialogDescription>
      </DialogHeader>

      <form class="grid gap-3" @submit.prevent="submit">
        <div class="grid gap-1.5">
          <Label for="stash-message" class="text-base">Message <span class="text-muted-foreground">(optional)</span></Label>
          <Input
            id="stash-message"
            v-model="message"
            class="text-base"
            placeholder="Defaults to the last commit's subject"
            autocomplete="off"
            spellcheck="true"
          />
        </div>

        <div class="grid gap-1">
          <label class="flex items-center gap-2 text-base select-none" :class="request.untrackedCount === 0 && 'opacity-50'">
            <input v-model="includeUntracked" type="checkbox" class="size-3.5 accent-primary" :disabled="request.untrackedCount === 0" />
            Include untracked files
            <span class="text-muted-foreground">({{ request.untrackedCount }})</span>
          </label>
          <p class="text-xs text-muted-foreground">
            {{ filesText(request.trackedCount) }} with changes<template v-if="includeUntracked && request.untrackedCount > 0">
              and {{ filesText(request.untrackedCount) }} not tracked yet</template> will be stashed.
            <template v-if="!includeUntracked && request.untrackedCount > 0">Untracked files stay in the working tree.</template>
          </p>
          <p v-if="nothingToSave" class="text-xs text-modified-fg">Only untracked files changed, so include them to stash anything.</p>
        </div>

        <p v-if="error" class="rounded-md border border-destructive/40 bg-destructive/10 px-2.5 py-1.5 text-base break-words text-destructive">
          {{ error }}
        </p>
      </form>

      <DialogFooter>
        <Button type="button" variant="outline" @click="dialogs.cancel()">Cancel</Button>
        <Button type="button" :disabled="!canSubmit" @click="submit">
          <LoaderCircleIcon v-if="submitting" class="animate-spin" />
          Stash
        </Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
</template>
