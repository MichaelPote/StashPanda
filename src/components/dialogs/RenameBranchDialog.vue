<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { LoaderCircleIcon } from "@lucide/vue";
import { commands } from "@/lib/bindings";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { errorMessage } from "@/lib/ipc";
import { validateRefName } from "@/lib/refNames";
import { opFailureText, runOp } from "@/lib/repoActions";
import { useDialogsStore } from "@/stores/dialogs";
import { useSessionsStore } from "@/stores/sessions";

const dialogs = useDialogsStore();
const sessions = useSessionsStore();

const request = computed(() => dialogs.current("renameBranch"));

const name = ref("");
const error = ref<string | null>(null);
const submitting = ref(false);

watch(request, (r) => {
  if (!r) return;
  name.value = r.name;
  error.value = null;
  submitting.value = false;
});

const trimmed = computed(() => name.value.trim());
const nameProblem = computed(() => (trimmed.value.length === 0 ? null : validateRefName(trimmed.value, "branch")));
const canSubmit = computed(
  () => trimmed.value.length > 0 && trimmed.value !== request.value?.name && nameProblem.value === null && !submitting.value,
);

function onOpenChange(open: boolean): void {
  if (!open) dialogs.cancel();
}

async function submit(): Promise<void> {
  const r = request.value;
  if (!r || !canSubmit.value) return;
  const to = trimmed.value;
  submitting.value = true;
  error.value = null;
  try {
    const result = await runOp(r.key, () => commands.opBranchRename(r.key, r.name, to));
    if (!result.ok) {
      error.value = opFailureText(result, `Renaming ${r.name} to ${to}`);
      return;
    }
    sessions.notifySuccess(`Renamed branch ${r.name} to ${to}.`);
    dialogs.resolve("renameBranch", result);
  } catch (e) {
    error.value = errorMessage(e);
  } finally {
    submitting.value = false;
  }
}
</script>

<template>
  <Dialog :open="request !== null" @update:open="onOpenChange">
    <DialogContent v-if="request" class="sm:max-w-lg" size-key="renameBranch">
      <DialogHeader>
        <DialogTitle>Rename branch</DialogTitle>
        <DialogDescription>
          <span class="font-mono">{{ request.name }}</span> keeps its commits and the remote branch it tracks. A copy on a remote keeps
          the old name.
        </DialogDescription>
      </DialogHeader>

      <form class="grid gap-3" @submit.prevent="submit">
        <div class="grid gap-1.5">
          <Label for="rename-branch-name" class="text-base">New name</Label>
          <Input
            id="rename-branch-name"
            v-model="name"
            class="font-mono text-base"
            autocomplete="off"
            spellcheck="false"
            :aria-invalid="nameProblem !== null || undefined"
          />
          <p v-if="nameProblem" class="text-xs text-destructive">{{ nameProblem }}</p>
        </div>

        <p v-if="error" class="rounded-md border border-destructive/40 bg-destructive/10 px-2.5 py-1.5 text-base break-words text-destructive">
          {{ error }}
        </p>
      </form>

      <DialogFooter>
        <Button type="button" variant="outline" @click="dialogs.cancel()">Cancel</Button>
        <Button type="button" :disabled="!canSubmit" @click="submit">
          <LoaderCircleIcon v-if="submitting" class="animate-spin" />
          Rename
        </Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
</template>
