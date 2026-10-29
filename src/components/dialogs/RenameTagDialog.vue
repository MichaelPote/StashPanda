<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { LoaderCircleIcon } from "@lucide/vue";
import { commands } from "@/lib/bindings";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import RemoteTagOptions from "@/components/dialogs/RemoteTagOptions.vue";
import { errorMessage, unwrap } from "@/lib/ipc";
import { validateRefName } from "@/lib/refNames";
import { opFailureText, runOp } from "@/lib/repoActions";
import { isSignedTagMessage } from "@/lib/tagAnnotations";
import { type RemoteTagChoice, useDialogsStore } from "@/stores/dialogs";
import { useSessionsStore } from "@/stores/sessions";

/** Renames a tag here; the remotes ticked are renamed afterwards by `renameTag`. */
const dialogs = useDialogsStore();
const sessions = useSessionsStore();

const request = computed(() => dialogs.current("renameTag"));

const name = ref("");
const remotes = ref<RemoteTagChoice[]>([]);
/** The tag carries a signature, which the renamed tag cannot keep. */
const signed = ref(false);
const error = ref<string | null>(null);
const submitting = ref(false);

watch(request, (r) => {
  if (!r) return;
  name.value = r.name;
  remotes.value = [];
  signed.value = false;
  error.value = null;
  submitting.value = false;
  unwrap(commands.getTagAnnotation(r.key, r.name))
    .then((a) => {
      if (request.value === r) signed.value = a !== null && isSignedTagMessage(a.message);
    })
    .catch(() => {
      // No warning, then.
    });
});

const tags = computed(() => {
  const r = request.value;
  return r ? (sessions.sessions.get(r.key)?.refs?.refs ?? []).filter((e) => e.kind.type === "tag") : [];
});
const commit = computed(() => tags.value.find((e) => e.short === request.value?.name)?.target ?? "");

const trimmed = computed(() => name.value.trim());
const nameProblem = computed(() => {
  if (trimmed.value.length === 0 || trimmed.value === request.value?.name) return null;
  if (tags.value.some((e) => e.short === trimmed.value)) return `A tag ${trimmed.value} already exists.`;
  return validateRefName(trimmed.value, "tag");
});
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
    const result = await runOp(r.key, () => commands.opTagRename(r.key, r.name, to));
    if (!result.ok) {
      error.value = opFailureText(result, `Renaming ${r.name} to ${to}`);
      return;
    }
    // The caller goes on to the remotes, and says how that went.
    if (remotes.value.length === 0) sessions.notifySuccess(`Renamed tag ${r.name} to ${to}.`);
    dialogs.resolve("renameTag", { name: to, remotes: remotes.value });
  } catch (e) {
    error.value = errorMessage(e);
  } finally {
    submitting.value = false;
  }
}
</script>

<template>
  <Dialog :open="request !== null" @update:open="onOpenChange">
    <DialogContent v-if="request" class="sm:max-w-lg" size-key="renameTag">
      <DialogHeader>
        <DialogTitle>Rename tag</DialogTitle>
        <DialogDescription>
          <span class="font-mono">{{ request.name }}</span> stays on the same commit, with the same message, tagger and date.
        </DialogDescription>
      </DialogHeader>

      <form class="grid gap-3" @submit.prevent="submit">
        <div class="grid gap-1.5">
          <Label for="rename-tag-name" class="text-base">New name</Label>
          <Input
            id="rename-tag-name"
            v-model="name"
            class="font-mono text-base"
            autocomplete="off"
            spellcheck="false"
            :aria-invalid="nameProblem !== null || undefined"
          />
          <p v-if="nameProblem" class="text-xs text-destructive">{{ nameProblem }}</p>
        </div>

        <p v-if="signed" class="rounded-md border border-modified/40 bg-modified/10 px-2.5 py-1.5 text-base text-modified-fg" data-rename-tag-signed>
          {{ request.name }} is signed. The signature covers the tag's name, so the renamed tag cannot keep it and will not be signed.
        </p>

        <RemoteTagOptions
          v-model="remotes"
          :session-key="request.key"
          action="rename"
          :tag="request.name"
          :commit="commit"
          :renamed="trimmed !== request.name ? trimmed : ''"
        />

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
