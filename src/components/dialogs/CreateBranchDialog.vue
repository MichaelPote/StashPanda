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

const request = computed(() => dialogs.current("createBranch"));

const name = ref("");
const checkout = ref(true);
const error = ref<string | null>(null);
const submitting = ref(false);

watch(request, (r) => {
  if (!r) return;
  name.value = "";
  checkout.value = true;
  error.value = null;
  submitting.value = false;
});

const trimmed = computed(() => name.value.trim());
/** Shown while typing, but only once there is something to check. */
const nameProblem = computed(() => (trimmed.value.length === 0 ? null : validateRefName(trimmed.value, "branch")));
const canSubmit = computed(() => trimmed.value.length > 0 && nameProblem.value === null && !submitting.value);

function onOpenChange(open: boolean): void {
  if (!open) dialogs.cancel();
}

async function submit(): Promise<void> {
  const r = request.value;
  if (!r || !canSubmit.value) return;
  const branch = trimmed.value;
  submitting.value = true;
  error.value = null;
  try {
    const result = await runOp(r.key, commands.opBranchCreate(r.key, branch, r.startPoint, checkout.value));
    if (!result.ok) {
      error.value = opFailureText(result, `Creating branch ${branch}`);
      return;
    }
    sessions.notify(checkout.value ? `Created branch ${branch} and checked it out.` : `Created branch ${branch}.`);
    dialogs.resolve("createBranch", result);
  } catch (e) {
    error.value = errorMessage(e);
  } finally {
    submitting.value = false;
  }
}
</script>

<template>
  <Dialog :open="request !== null" @update:open="onOpenChange">
    <DialogContent v-if="request" class="sm:max-w-md">
      <DialogHeader>
        <DialogTitle>Create branch</DialogTitle>
        <DialogDescription>
          The new branch starts at <span class="font-mono">{{ request.startLabel }}</span>.
        </DialogDescription>
      </DialogHeader>

      <form class="grid gap-3" @submit.prevent="submit">
        <div class="grid gap-1.5">
          <Label for="create-branch-name" class="text-base">Branch name</Label>
          <Input
            id="create-branch-name"
            v-model="name"
            class="font-mono text-base"
            placeholder="feature/my-change"
            autocomplete="off"
            spellcheck="false"
            :aria-invalid="nameProblem !== null || undefined"
          />
          <p v-if="nameProblem" class="text-xs text-destructive">{{ nameProblem }}</p>
        </div>

        <label class="flex items-center gap-2 text-base select-none">
          <input v-model="checkout" type="checkbox" class="size-3.5 accent-primary" />
          Check out the new branch
        </label>

        <p v-if="error" class="rounded-md border border-destructive/40 bg-destructive/10 px-2.5 py-1.5 text-base break-words text-destructive">
          {{ error }}
        </p>
      </form>

      <DialogFooter>
        <Button type="button" variant="outline" size="sm" @click="dialogs.cancel()">Cancel</Button>
        <Button type="button" size="sm" :disabled="!canSubmit" @click="submit">
          <LoaderCircleIcon v-if="submitting" class="animate-spin" />
          Create
        </Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
</template>
