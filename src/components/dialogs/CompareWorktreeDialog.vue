<script setup lang="ts">
import { computed } from "vue";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import DiffViewer from "@/components/diff/DiffViewer.vue";
import { revisionLabel } from "@/lib/blame";
import { showCommitInGraph } from "@/lib/fileViews";
import { splitPath } from "@/lib/files";
import { useDialogsStore } from "@/stores/dialogs";
import { useSessionsStore, type SelectedFile } from "@/stores/sessions";

/**
 * One file as it is in a commit, compared with the file in the working tree now. Opened from the
 * right-click menu of a file in a commit's list. Modelled on the File History dialog: the same
 * diff viewer, here with one fixed comparison in place of a list of commits.
 */
const dialogs = useDialogsStore();
const sessions = useSessionsStore();

const request = computed(() => dialogs.current("compareWorktree"));
const session = computed(() => (request.value ? (sessions.sessions.get(request.value.key) ?? null) : null));

/** The commit's version is the old side and the working tree's the new one. */
const file = computed<SelectedFile | null>(() => {
  const r = request.value;
  return r ? { path: r.path, oldPath: null, target: { type: "commitVsWorktree", commit: r.commit } } : null;
});

const name = computed(() => (request.value ? splitPath(request.value.path).name : ""));
const commitLabel = computed(() => (request.value ? revisionLabel(request.value.commit) : ""));

function onOpenChange(open: boolean): void {
  if (!open) dialogs.cancel();
}

function showInGraph(): void {
  const r = request.value;
  if (r) void showCommitInGraph(r.key, r.commit);
}
</script>

<template>
  <Dialog :open="request !== null" @update:open="onOpenChange">
    <DialogContent v-if="request" class="h-[80vh] sm:max-w-[80vw]" size-key="compareWorktree">
      <DialogHeader>
        <DialogTitle>Compare {{ name }} with the Working Tree</DialogTitle>
        <DialogDescription class="break-all">
          {{ request.path }} as it is in commit {{ commitLabel }}, compared with the file in the working tree now. Removed lines are only in the commit;
          added lines are only in the working tree.
        </DialogDescription>
      </DialogHeader>

      <!-- On the window colour like the main window's panels, as in the File History dialog. -->
      <div class="-mx-6 -my-5 flex min-h-0 flex-1 flex-col bg-window p-1.5" data-dialog-grow>
        <DiffViewer v-if="session" :session="session" :file="file" :file-actions="false" />
      </div>

      <DialogFooter>
        <Button type="button" variant="outline" @click="showInGraph">Show Commit in Commit Graph</Button>
        <Button type="button" @click="dialogs.cancel()">Close</Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
</template>
