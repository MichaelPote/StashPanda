<script setup lang="ts">
import { computed, ref, watch } from "vue";
import type { StashEntry } from "@/lib/bindings";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { formatRelativeTime } from "@/lib/format";
import { stashParts, stashTitle } from "@/lib/stash";
import { cn } from "@/lib/utils";
import { useDialogsStore } from "@/stores/dialogs";
import { useSessionsStore } from "@/stores/sessions";

const dialogs = useDialogsStore();
const sessions = useSessionsStore();

const request = computed(() => dialogs.current("stashApply"));
/** Read live from the session, so a stash made or dropped while the dialog is open shows up. */
const stashes = computed<StashEntry[]>(() => (request.value ? (sessions.sessions.get(request.value.key)?.stashes ?? []) : []));

const pickedOid = ref<string | null>(null);
const drop = ref(false);

watch(request, (r) => {
  if (!r) return;
  pickedOid.value = stashes.value[0]?.oid ?? null;
  drop.value = false;
});

/**
 * The picked stash, or null when nothing is picked or the picked stash has gone from the list
 * (dropped or popped elsewhere while the dialog was open). It never falls back to another stash,
 * which Apply would then apply without the user having chosen it.
 */
const picked = computed(() => stashes.value.find((s) => s.oid === pickedOid.value) ?? null);
const pickedGone = computed(() => pickedOid.value !== null && picked.value === null);

function onOpenChange(open: boolean): void {
  if (!open) dialogs.cancel();
}

function submit(): void {
  const stash = picked.value;
  if (!stash) return;
  // The caller runs the command, the same one the Branches panel's stash menu uses.
  dialogs.resolve("stashApply", { stash, drop: drop.value });
}
</script>

<template>
  <Dialog :open="request !== null" @update:open="onOpenChange">
    <DialogContent v-if="request" class="sm:max-w-xl" size-key="applyStash">
      <DialogHeader>
        <DialogTitle>Apply stash</DialogTitle>
        <DialogDescription>Puts a stash's changes back into the working tree.</DialogDescription>
      </DialogHeader>

      <form class="grid gap-3" data-dialog-grow @submit.prevent="submit">
        <div
          role="radiogroup"
          aria-label="Stash to apply"
          data-dialog-grow
          style="--grow-basis: 18rem"
          class="max-h-72 overflow-auto rounded-lg border border-border well py-0.5 scrollbar-thin"
        >
          <p v-if="stashes.length === 0" class="flex h-(--row-h) items-center px-2 text-base text-muted-foreground italic">No stashes</p>
          <label
            v-for="s in stashes"
            :key="s.oid"
            :class="
              cn(
                'flex h-(--row-h) min-w-0 cursor-default items-center gap-2 px-2 text-base select-none',
                picked?.oid === s.oid ? 'row-selected' : 'hover:bg-row-hover',
              )
            "
            :title="stashTitle(s)"
          >
            <input v-model="pickedOid" type="radio" name="apply-stash" class="size-3.5 shrink-0 accent-primary" :value="s.oid" />
            <span class="shrink-0 font-mono text-xs text-muted-foreground">{{ s.selector }}</span>
            <span class="min-w-0 flex-1 truncate">{{ stashParts(s).text }}</span>
            <span class="shrink-0 text-[0.7rem] text-muted-foreground tabular-nums">{{ formatRelativeTime(s.time) }}</span>
          </label>
        </div>

        <p v-if="pickedGone" class="text-base text-destructive">The stash you picked is no longer there. Choose one from the list.</p>

        <div class="grid gap-1">
          <label class="flex items-center gap-2 text-base select-none">
            <input v-model="drop" type="checkbox" class="size-3.5 accent-primary" />
            Drop the stash after applying it
          </label>
          <p class="text-xs text-muted-foreground">
            If applying it causes conflicts, Git keeps the stash anyway, so nothing is lost.
          </p>
        </div>
      </form>

      <DialogFooter>
        <Button type="button" variant="outline" @click="dialogs.cancel()">Cancel</Button>
        <Button type="button" :disabled="picked === null" @click="submit">{{ drop ? "Apply and drop" : "Apply" }}</Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
</template>
