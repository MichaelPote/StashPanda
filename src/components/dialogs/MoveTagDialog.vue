<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { LoaderCircleIcon } from "@lucide/vue";
import { commands, type GraphRow, type TagAnnotation } from "@/lib/bindings";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Label } from "@/components/ui/label";
import CommitPicker from "@/components/dialogs/CommitPicker.vue";
import RemoteTagOptions from "@/components/dialogs/RemoteTagOptions.vue";
import { errorMessage, unwrap } from "@/lib/ipc";
import { shortSha } from "@/lib/refNames";
import { opFailureText, runOp } from "@/lib/repoActions";
import { isSignedTagMessage } from "@/lib/tagAnnotations";
import { type RemoteTagChoice, useDialogsStore } from "@/stores/dialogs";
import { useSessionsStore } from "@/stores/sessions";

/** Moves a tag here to a commit chosen from a list; the remotes ticked are moved afterwards by `moveTag`. */
const dialogs = useDialogsStore();
const sessions = useSessionsStore();

const request = computed(() => dialogs.current("moveTag"));

const target = ref<GraphRow | null>(null);
const remotes = ref<RemoteTagChoice[]>([]);
const annotation = ref<TagAnnotation | null>(null);
const error = ref<string | null>(null);
const submitting = ref(false);

watch(request, (r) => {
  if (!r) return;
  target.value = null;
  remotes.value = [];
  annotation.value = null;
  error.value = null;
  submitting.value = false;
  unwrap(commands.getTagAnnotation(r.key, r.name))
    .then((a) => {
      if (request.value === r) annotation.value = a;
    })
    .catch(() => {
      // No note about the message, then.
    });
});

/** The tag as the refs list has it: the commit it is on now. */
const entry = computed(() => {
  const r = request.value;
  return r ? ((sessions.sessions.get(r.key)?.refs?.refs ?? []).find((e) => e.kind.type === "tag" && e.short === r.name) ?? null) : null;
});
const commit = computed(() => entry.value?.target ?? "");

const targetProblem = computed(() =>
  target.value !== null && target.value.oid === commit.value ? `${request.value?.name} is already on this commit.` : null,
);
const canSubmit = computed(() => target.value !== null && targetProblem.value === null && !submitting.value);

function onOpenChange(open: boolean): void {
  if (!open) dialogs.cancel();
}

async function submit(): Promise<void> {
  const r = request.value;
  const to = target.value;
  if (!r || !to || !canSubmit.value) return;
  const label = shortSha(to.oid);
  submitting.value = true;
  error.value = null;
  try {
    const result = await runOp(r.key, () => commands.opTagMove(r.key, r.name, to.oid));
    if (!result.ok) {
      error.value = opFailureText(result, `Moving ${r.name} to ${label}`);
      return;
    }
    // The caller goes on to the remotes, and says how that went.
    if (remotes.value.length === 0) sessions.notifySuccess(`Moved tag ${r.name} to ${label}.`);
    dialogs.resolve("moveTag", { targetLabel: label, remotes: remotes.value });
  } catch (e) {
    error.value = errorMessage(e);
  } finally {
    submitting.value = false;
  }
}
</script>

<template>
  <Dialog :open="request !== null" @update:open="onOpenChange">
    <DialogContent v-if="request" class="sm:max-w-xl" size-key="moveTag">
      <DialogHeader>
        <DialogTitle>Move tag {{ request.name }}</DialogTitle>
        <DialogDescription>
          <span class="font-mono">{{ request.name }}</span> is on <span class="font-mono">{{ shortSha(commit) }}</span>
          <template v-if="entry?.subject"> ({{ entry.subject }})</template>. It is removed from there and put on the commit you choose.
        </DialogDescription>
      </DialogHeader>

      <form class="grid gap-3" @submit.prevent="submit">
        <div class="grid gap-1.5">
          <Label for="move-tag-target" class="text-base">Move to</Label>
          <CommitPicker id="move-tag-target" v-model="target" :session-key="request.key" />
          <p v-if="targetProblem" class="text-xs text-destructive">{{ targetProblem }}</p>
        </div>

        <p v-if="annotation?.annotated" class="text-xs text-muted-foreground">
          The tag keeps its message. As with <span class="font-mono">git tag -f</span>, you become its tagger, with today's date.
        </p>
        <p
          v-if="annotation?.annotated && isSignedTagMessage(annotation.message)"
          class="rounded-md border border-modified/40 bg-modified/10 px-2.5 py-1.5 text-base text-modified-fg"
          data-move-tag-signed
        >
          {{ request.name }} is signed. The signature covers the commit it is on, so the moved tag cannot keep it and will not be signed.
        </p>

        <RemoteTagOptions v-model="remotes" :session-key="request.key" action="move" :tag="request.name" :commit="commit" />

        <p v-if="error" class="rounded-md border border-destructive/40 bg-destructive/10 px-2.5 py-1.5 text-base break-words text-destructive">
          {{ error }}
        </p>
      </form>

      <DialogFooter>
        <Button type="button" variant="outline" @click="dialogs.cancel()">Cancel</Button>
        <Button type="button" :disabled="!canSubmit" @click="submit">
          <LoaderCircleIcon v-if="submitting" class="animate-spin" />
          Move
        </Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
</template>
