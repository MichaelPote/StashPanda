<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { useDialogsStore } from "@/stores/dialogs";

/**
 * Confirms deleting a local branch, and offers to delete the remote branch it tracks as well.
 * `deleteBranch` in `lib/repoActions.ts` works out what can be offered and does the deleting.
 */
const dialogs = useDialogsStore();

const request = computed(() => dialogs.current("deleteBranch"));
const deleteTracked = ref(false);

// Unticked each time: the tracked branch is deleted for everyone who uses the remote.
watch(request, () => {
  deleteTracked.value = false;
});

function onOpenChange(open: boolean): void {
  if (!open) dialogs.cancel();
}

function confirm(): void {
  const r = request.value;
  if (r) dialogs.resolve("deleteBranch", { deleteTracked: deleteTracked.value && r.tracked !== null });
}
</script>

<template>
  <Dialog :open="request !== null" @update:open="onOpenChange">
    <DialogContent v-if="request" class="sm:max-w-lg" size-key="deleteBranch">
      <DialogHeader>
        <DialogTitle>Delete branch {{ request.name }}?</DialogTitle>
      </DialogHeader>

      <div class="grid gap-1">
        <label :class="['flex items-center gap-2 text-base select-none', request.tracked ? '' : 'text-muted-foreground']">
          <input v-model="deleteTracked" type="checkbox" class="size-3.5 accent-primary" :disabled="request.tracked === null" data-delete-tracked />
          <template v-if="request.trackedShort">
            Also delete <span class="font-mono">{{ request.trackedShort }}</span>
          </template>
          <template v-else>Also delete tracked branch</template>
        </label>
        <!-- Red when it is a warning: about the remote, or that a tracked branch of another name is
             left there. Grey when it only says there is nothing to delete. It is also the dialog's
             description, which is what a screen reader reads out on opening. -->
        <DialogDescription
          :class="['text-xs', request.tracked || request.trackedReasonIsWarning ? 'text-destructive' : 'text-muted-foreground']"
          data-delete-tracked-note
        >
          <template v-if="request.tracked">
            Once the local branch is deleted, <span class="font-mono">{{ request.tracked.branch }}</span> is deleted on
            {{ request.tracked.remote }} too, for everyone who uses that remote. Commits that no other branch there contains can then be
            lost. Nothing is deleted there if it has commits this repository has not fetched.
          </template>
          <template v-else>{{ request.trackedReason }}</template>
        </DialogDescription>
      </div>

      <DialogFooter>
        <Button type="button" variant="outline" @click="dialogs.cancel()">Cancel</Button>
        <Button type="button" variant="destructive" autofocus @click="confirm">Delete branch</Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
</template>
