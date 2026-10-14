<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { LoaderCircleIcon } from "@lucide/vue";
import { commands } from "@/lib/bindings";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Textarea } from "@/components/ui/textarea";
import { errorMessage } from "@/lib/ipc";
import { validateRefName } from "@/lib/refNames";
import { opFailureText, runOp } from "@/lib/repoActions";
import { useDialogsStore } from "@/stores/dialogs";
import { useSessionsStore } from "@/stores/sessions";

const dialogs = useDialogsStore();
const sessions = useSessionsStore();

const request = computed(() => dialogs.current("createTag"));

const name = ref("");
const message = ref("");
const force = ref(false);
const error = ref<string | null>(null);
const submitting = ref(false);

watch(request, (r) => {
  if (!r) return;
  name.value = "";
  message.value = "";
  force.value = false;
  error.value = null;
  submitting.value = false;
});

const trimmed = computed(() => name.value.trim());
const nameProblem = computed(() => (trimmed.value.length === 0 ? null : validateRefName(trimmed.value, "tag")));
const canSubmit = computed(() => trimmed.value.length > 0 && nameProblem.value === null && !submitting.value);

function onOpenChange(open: boolean): void {
  if (!open) dialogs.cancel();
}

async function submit(): Promise<void> {
  const r = request.value;
  if (!r || !canSubmit.value) return;
  const tag = trimmed.value;
  // A message makes the tag annotated (`git tag -a -m`); none makes it lightweight.
  const text = message.value.trim();
  submitting.value = true;
  error.value = null;
  try {
    const result = await runOp(r.key, () => commands.opTagCreate(r.key, tag, r.target, text.length > 0 ? text : null, force.value));
    if (!result.ok) {
      error.value = opFailureText(result, `Creating tag ${tag}`);
      return;
    }
    sessions.notifySuccess(`Created ${text.length > 0 ? "annotated " : ""}tag ${tag} at ${r.targetLabel}.`);
    dialogs.resolve("createTag", result);
  } catch (e) {
    error.value = errorMessage(e);
  } finally {
    submitting.value = false;
  }
}
</script>

<template>
  <Dialog :open="request !== null" @update:open="onOpenChange">
    <DialogContent v-if="request" class="sm:max-w-lg" size-key="createTag">
      <DialogHeader>
        <DialogTitle>Create tag</DialogTitle>
        <DialogDescription>
          The tag points at <span class="font-mono">{{ request.targetLabel }}</span>.
        </DialogDescription>
      </DialogHeader>

      <form class="grid gap-3" data-dialog-grow @submit.prevent="submit">
        <div class="grid gap-1.5">
          <Label for="create-tag-name" class="text-base">Tag name</Label>
          <Input
            id="create-tag-name"
            v-model="name"
            class="font-mono text-base"
            placeholder="v1.2.0"
            autocomplete="off"
            spellcheck="false"
            :aria-invalid="nameProblem !== null || undefined"
          />
          <p v-if="nameProblem" class="text-xs text-destructive">{{ nameProblem }}</p>
        </div>

        <div class="grid gap-1.5" data-dialog-grow>
          <Label for="create-tag-message" class="text-base">
            Message <span class="text-muted-foreground">(optional; makes the tag annotated)</span>
          </Label>
          <Textarea id="create-tag-message" v-model="message" data-dialog-grow style="--grow-basis: 4rem" class="min-h-16 text-base" placeholder="Release 1.2.0" spellcheck="true" />
        </div>

        <label class="flex items-center gap-2 text-base select-none">
          <input v-model="force" type="checkbox" class="size-3.5 accent-primary" />
          Replace an existing tag with this name
        </label>

        <p v-if="error" class="rounded-md border border-destructive/40 bg-destructive/10 px-2.5 py-1.5 text-base break-words text-destructive">
          {{ error }}
        </p>
      </form>

      <DialogFooter>
        <Button type="button" variant="outline" @click="dialogs.cancel()">Cancel</Button>
        <Button type="button" :disabled="!canSubmit" @click="submit">
          <LoaderCircleIcon v-if="submitting" class="animate-spin" />
          Create
        </Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
</template>
