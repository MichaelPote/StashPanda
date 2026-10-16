<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { useDialogsStore } from "@/stores/dialogs";
import { useSessionsStore } from "@/stores/sessions";

const dialogs = useDialogsStore();
const sessions = useSessionsStore();

const request = computed(() => dialogs.current("push"));

const remote = ref("");
const remoteBranch = ref("");
const setUpstream = ref(true);

watch(request, (r) => {
  if (!r) return;
  remote.value = r.remotes.includes("origin") ? "origin" : (r.remotes[0] ?? "");
  remoteBranch.value = r.branch;
  // A branch that tracks a remote branch already keeps tracking it unless the user asks.
  setUpstream.value = r.tracks === null;
});

/** Why the remote branch name cannot be used, or null. The backend checks it again. */
const nameProblem = computed(() => {
  const name = remoteBranch.value.trim();
  if (name.length === 0) return "Type the branch name to use on the remote.";
  if (name.startsWith("-") || name.includes("..") || name.endsWith("/") || name.endsWith(".lock") || /[\s~^:?*[\\]/.test(name)) {
    return "That is not a valid branch name.";
  }
  return null;
});
const canSubmit = computed(() => request.value !== null && remote.value !== "" && nameProblem.value === null);

/**
 * True when the chosen remote already has a branch of the name typed, as last fetched. Pushing
 * there adds to someone's existing branch rather than creating one, so the dialog says so and the
 * button names it. Git still only lets a fast-forward through.
 */
const exists = computed(() => {
  const r = request.value;
  const name = remoteBranch.value.trim();
  if (!r || name === "") return false;
  const refs = sessions.sessions.get(r.key)?.refs?.refs ?? [];
  return refs.some((x) => x.kind.type === "remoteBranch" && x.kind.remote === remote.value && x.short === `${remote.value}/${name}`);
});

const selectClass =
  "well h-8 w-full rounded-lg border px-2.5 text-base outline-none focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/50";

function onOpenChange(open: boolean): void {
  if (!open) dialogs.cancel();
}

/** The caller runs the push, the same one the toolbar's Push button runs. */
function submit(): void {
  if (!canSubmit.value) return;
  dialogs.resolve("push", { target: { remote: remote.value, branch: remoteBranch.value.trim() }, setUpstream: setUpstream.value });
}
</script>

<template>
  <Dialog :open="request !== null" @update:open="onOpenChange">
    <DialogContent v-if="request" class="sm:max-w-lg" size-key="push">
      <DialogHeader>
        <DialogTitle>Push {{ request.branch }}</DialogTitle>
        <DialogDescription v-if="request.tracks">
          <span class="font-mono">{{ request.branch }}</span> pulls from <span class="font-mono">{{ request.tracks }}</span>, a branch with a
          different name. Choose where to push it.
        </DialogDescription>
        <DialogDescription v-else>
          <span class="font-mono">{{ request.branch }}</span> does not track a remote branch yet. Choose where to push it.
        </DialogDescription>
      </DialogHeader>

      <form class="grid gap-3" @submit.prevent="submit">
        <div class="grid gap-1.5">
          <Label for="push-remote" class="text-base">Remote</Label>
          <select id="push-remote" v-model="remote" :class="selectClass">
            <option v-for="r in request.remotes" :key="r" :value="r">{{ r }}</option>
          </select>
        </div>
        <div class="grid gap-1.5">
          <Label for="push-branch" class="text-base">Branch name on the remote</Label>
          <Input id="push-branch" v-model="remoteBranch" class="font-mono text-base" autocomplete="off" spellcheck="false" />
          <p v-if="nameProblem" class="text-xs text-modified-fg">{{ nameProblem }}</p>
          <p v-else-if="exists" class="text-xs text-modified-fg" data-push-existing>
            {{ remote }} already has a branch {{ remoteBranch.trim() }}. Pushing adds {{ request.branch }}'s commits to it, for everyone who uses it,
            when that is a fast-forward; Git refuses otherwise.
          </p>
        </div>
        <div class="grid gap-1">
          <label class="flex items-center gap-2 text-base select-none">
            <input v-model="setUpstream" type="checkbox" class="size-3.5 accent-primary" />
            Track it from now on
          </label>
          <p class="text-xs text-muted-foreground">
            Sets <span class="font-mono">{{ remote }}/{{ remoteBranch.trim() || request.branch }}</span> as the upstream of
            <span class="font-mono">{{ request.branch }}</span>, so Pull and Push use it without asking.
          </p>
        </div>
      </form>

      <DialogFooter>
        <Button type="button" variant="outline" @click="dialogs.cancel()">Cancel</Button>
        <Button type="button" :disabled="!canSubmit" @click="submit">{{ exists ? "Push to Existing Branch" : "Push" }}</Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
</template>
