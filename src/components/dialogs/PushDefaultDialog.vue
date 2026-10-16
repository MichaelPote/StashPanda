<script setup lang="ts">
import { computed, ref, watch } from "vue";
import type { PushDefault } from "@/lib/bindings";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { cn } from "@/lib/utils";
import { useDialogsStore } from "@/stores/dialogs";

const dialogs = useDialogsStore();

const request = computed(() => dialogs.current("pushDefault"));

/** Push to the upstream this time only, or save one of the two `push.default` values that push to a branch with another name. */
type Picked = "once" | PushDefault;
const picked = ref<Picked>("once");

watch(request, (r) => {
  if (r) picked.value = "once";
});

interface Choice {
  id: Picked;
  label: string;
  detail: string;
  /** The `push.default` value the choice saves, named for anyone who also uses Git in a terminal. */
  setting: PushDefault | null;
}

/** Each choice says what happens to the branches; Git's own name for it is only a footnote. */
const choices = computed<Choice[]>(() => {
  const r = request.value;
  if (!r) return [];
  const { branch } = r;
  const { remote, branch: theirs } = r.upstream;
  return [
    {
      id: "once",
      label: `Push to ${theirs} this time only`,
      detail: `The commits on ${branch} are added to ${theirs} on ${remote}. Nothing is saved, so the next push asks again.`,
      setting: null,
    },
    {
      id: "upstream",
      label: "Always push to the branch it pulls from",
      detail: `From now on, every branch in this repository is pushed to the branch it pulls from, even when the names differ. ${branch} goes to ${theirs} on ${remote}.`,
      setting: "upstream",
    },
    {
      id: "current",
      label: "Always push to a branch with the same name",
      detail: `From now on, every branch in this repository is pushed to a branch with its own name, which is created on the remote if it is not there yet. ${branch} goes to ${branch} on ${remote}, and still pulls from ${theirs}.`,
      setting: "current",
    },
  ];
});

function onOpenChange(open: boolean): void {
  if (!open) dialogs.cancel();
}

function submit(): void {
  // The caller saves the setting and runs the push or sync, so its result shows in the usual notice.
  dialogs.resolve("pushDefault", picked.value === "once" ? { kind: "once" } : { kind: "always", mode: picked.value });
}
</script>

<template>
  <Dialog :open="request !== null" @update:open="onOpenChange">
    <DialogContent v-if="request" class="sm:max-w-xl" size-key="pushDefault">
      <DialogHeader>
        <DialogTitle>{{ request.action === "push" ? "Push" : "Sync" }} {{ request.branch }}</DialogTitle>
        <DialogDescription>Choose where its commits go.</DialogDescription>
      </DialogHeader>

      <form class="grid gap-3" @submit.prevent="submit">
        <p class="text-base">
          <span class="font-semibold">{{ request.branch }}</span> pulls from <span class="font-semibold">{{ request.upstream.branch }}</span> on
          {{ request.upstream.remote }}, a branch with a different name. Git does not push to it without asking, so that a branch started from
          {{ request.upstream.branch }} cannot put its commits on {{ request.upstream.branch }} by mistake.
        </p>
        <p v-if="request.action === 'sync'" class="text-xs text-muted-foreground">
          Sync first pulls from {{ request.upstream.branch }}, then pushes to the branch you choose here.
        </p>

        <div role="radiogroup" aria-label="Where to push" class="grid gap-1">
          <label
            v-for="c in choices"
            :key="c.id"
            :class="cn('flex items-start gap-2 rounded-md px-2 py-1.5 select-none', picked === c.id ? 'row-selected' : 'hover:bg-row-hover')"
          >
            <input v-model="picked" type="radio" name="push-default" class="mt-1 size-3.5 shrink-0 accent-primary" :value="c.id" />
            <span class="grid gap-0.5">
              <span class="text-base">{{ c.label }}</span>
              <span class="text-xs text-muted-foreground">{{ c.detail }}</span>
              <span v-if="c.setting" class="text-xs text-muted-foreground">
                Saved in this repository's Git settings as <span class="font-mono">push.default = {{ c.setting }}</span>, so a push from a terminal here does the same.
              </span>
            </span>
          </label>
        </div>
      </form>

      <DialogFooter>
        <Button v-if="request.action === 'push'" type="button" variant="outline" class="sm:mr-auto" @click="dialogs.resolve('pushDefault', { kind: 'otherTarget' })">
          Choose Another Branch…
        </Button>
        <Button type="button" variant="outline" @click="dialogs.cancel()">Cancel</Button>
        <Button type="button" @click="submit">{{ request.action === "push" ? "Push" : "Sync" }}</Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
</template>
