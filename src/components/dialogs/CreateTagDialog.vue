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
import { shortSha, validateRefName } from "@/lib/refNames";
import { defaultTagRemote, opFailureText, pushTags, runOp } from "@/lib/repoActions";
import { useDialogsStore } from "@/stores/dialogs";
import { useSessionsStore } from "@/stores/sessions";

const dialogs = useDialogsStore();
const sessions = useSessionsStore();

const request = computed(() => dialogs.current("createTag"));

const name = ref("");
const message = ref("");
/** Move an existing tag of this name here (`git tag -f`). Offered only when the name is taken. */
const force = ref(false);
/** Push the new tag once it is created. Off by default: a tag stays in this repository until pushed, as in Git. */
const push = ref(false);
const error = ref<string | null>(null);
const submitting = ref(false);

/** Where the tag is pushed when Push is ticked; null when the repository has no remotes, which hides the box. */
const remote = computed(() => (request.value ? defaultTagRemote(sessions.sessions.get(request.value.key)) : null));

watch(request, (r) => {
  if (!r) return;
  name.value = "";
  message.value = "";
  force.value = false;
  push.value = false;
  error.value = null;
  submitting.value = false;
});

const trimmed = computed(() => name.value.trim());
const nameProblem = computed(() => (trimmed.value.length === 0 ? null : validateRefName(trimmed.value, "tag")));

/** The tag that already has this name, which Create would refuse unless it is moved here. */
const existing = computed(() => {
  const r = request.value;
  if (!r || trimmed.value.length === 0) return null;
  return (sessions.sessions.get(r.key)?.refs?.refs ?? []).find((e) => e.kind.type === "tag" && e.short === trimmed.value) ?? null;
});
const canSubmit = computed(() => trimmed.value.length > 0 && nameProblem.value === null && (existing.value === null || force.value) && !submitting.value);

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
    const moved = existing.value !== null && force.value;
    const result = await runOp(r.key, () => commands.opTagCreate(r.key, tag, r.target, text.length > 0 ? text : null, moved));
    if (!result.ok) {
      error.value = opFailureText(result, `Creating tag ${tag}`);
      return;
    }
    sessions.notifySuccess(
      moved ? `Moved tag ${tag} to ${r.targetLabel}.` : `Created ${text.length > 0 ? "annotated " : ""}tag ${tag} at ${r.targetLabel}.`,
    );
    const pushTo = push.value ? remote.value : null;
    dialogs.resolve("createTag", result);
    // After the dialog closes: the push shows its progress in the status bar like any other.
    if (pushTo) void pushTags(r.key, pushTo, { type: "named", names: [tag] }, tag);
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

        <div v-if="existing" class="grid gap-1">
          <label class="flex items-center gap-2 text-base select-none">
            <input v-model="force" type="checkbox" class="size-3.5 accent-primary" data-create-tag-move />
            <span>Move <span class="font-mono">{{ existing.short }}</span> here from <span class="font-mono">{{ shortSha(existing.target) }}</span></span>
          </label>
          <p class="text-xs text-muted-foreground">
            A tag <span class="font-mono">{{ existing.short }}</span> already exists on <span class="font-mono">{{ shortSha(existing.target) }}</span>
            <template v-if="existing.subject"> ({{ existing.subject }})</template>. Ticking this removes it from there and creates it on
            <span class="font-mono">{{ request.targetLabel }}</span> instead, with the message above. To keep it, choose another name.
          </p>
        </div>

        <div v-if="remote" class="grid gap-1">
          <label class="flex items-center gap-2 text-base select-none">
            <input v-model="push" type="checkbox" class="size-3.5 accent-primary" data-create-tag-push />
            Push to {{ remote }}
          </label>
          <p class="text-xs text-muted-foreground">
            Otherwise the tag stays in this repository until you push it. {{ remote }} refuses a tag it already has on another commit.
          </p>
        </div>

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
