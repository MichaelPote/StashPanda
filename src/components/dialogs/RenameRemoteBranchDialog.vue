<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { LoaderCircleIcon } from "@lucide/vue";
import { commands } from "@/lib/bindings";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { errorMessage, IpcError } from "@/lib/ipc";
import { validateRefName } from "@/lib/refNames";
import { branchesTracking, listNames, remoteRenameRefusal } from "@/lib/remoteBranchRename";
import { networkFailureText, runOp, runOperation } from "@/lib/repoActions";
import { useDialogsStore } from "@/stores/dialogs";
import { useSessionsStore } from "@/stores/sessions";

/**
 * Renames a branch on a remote, from the right-click menu of a remote branch in the Branches
 * panel or of its pill in the commit graph. The backend does it in one push that creates the new
 * name and deletes the old one together; afterwards the local branches that tracked the old name
 * are pointed at the new one.
 */
const dialogs = useDialogsStore();
const sessions = useSessionsStore();

const request = computed(() => dialogs.current("renameRemoteBranch"));
const session = computed(() => (request.value ? (sessions.sessions.get(request.value.key) ?? null) : null));

const name = ref("");
const error = ref<string | null>(null);
const submitting = ref(false);

watch(request, (r) => {
  if (!r) return;
  name.value = r.branch;
  error.value = null;
  submitting.value = false;
});

/** The remote branch as `origin/feature/x`, for the text. */
const short = computed(() => (request.value ? `${request.value.remote}/${request.value.branch}` : ""));
/** Local branches that track it, which follow it to the new name. */
const trackedBy = computed(() => (request.value ? branchesTracking(session.value?.refs?.refs ?? [], request.value.remote, request.value.branch) : []));

const trimmed = computed(() => name.value.trim());
const nameProblem = computed(() => {
  const r = request.value;
  if (!r || trimmed.value.length === 0 || trimmed.value === r.branch) return null;
  const invalid = validateRefName(trimmed.value, "branch");
  if (invalid) return invalid;
  const taken = (session.value?.refs?.refs ?? []).some((x) => x.fullName === `refs/remotes/${r.remote}/${trimmed.value}`);
  return taken ? `${r.remote} already has a branch ${trimmed.value}.` : null;
});
const canSubmit = computed(
  () => trimmed.value.length > 0 && trimmed.value !== request.value?.branch && nameProblem.value === null && !submitting.value,
);

function onOpenChange(open: boolean): void {
  // The push cannot be taken back by closing the dialog; Stop in the status bar ends it.
  if (!open && !submitting.value) dialogs.cancel();
}

async function submit(): Promise<void> {
  const r = request.value;
  if (!r || !canSubmit.value) return;
  const to = trimmed.value;
  // Read before the rename: afterwards these branches' upstream has gone.
  const followers = [...trackedBy.value];
  const what = `Renaming ${r.branch} to ${to} on ${r.remote}`;
  submitting.value = true;
  error.value = null;
  try {
    const result = await runOperation(r.key, what, (opId) => commands.opRenameRemoteBranch(r.key, opId, r.remote, r.branch, to));
    if (!result.ok) {
      error.value = remoteRenameRefusal(result, r.remote, r.branch, to) ?? networkFailureText(result, what);
      return;
    }
    const done = `Renamed ${r.branch} to ${to} on ${r.remote}.`;
    const lost: string[] = [];
    for (const local of followers) {
      try {
        const set = await runOp(r.key, () => commands.opBranchSetUpstream(r.key, local, `refs/remotes/${r.remote}/${to}`));
        if (!set.ok) lost.push(local);
      } catch {
        lost.push(local);
      }
    }
    const moved = followers.filter((f) => !lost.includes(f));
    const nowTrack = moved.length === 0 ? "" : ` ${listNames(moved)} now ${moved.length === 1 ? "tracks" : "track"} ${r.remote}/${to}.`;
    if (lost.length === 0) {
      sessions.notifySuccess(`${done}${nowTrack}`);
    } else {
      sessions.notify(
        `${done}${nowTrack} ${listNames(lost)} could not be made to track ${r.remote}/${to} and ${lost.length === 1 ? "tracks" : "track"} nothing now: use Track Remote Branch… on ${lost.length === 1 ? "it" : "each"}.`,
      );
    }
    dialogs.resolve("renameRemoteBranch", null);
  } catch (e) {
    error.value =
      e instanceof IpcError && e.error.kind === "cancelled"
        ? `The rename was stopped. ${r.remote} may already have made it; the next fetch shows whether it did.`
        : errorMessage(e);
  } finally {
    submitting.value = false;
  }
}
</script>

<template>
  <Dialog :open="request !== null" @update:open="onOpenChange">
    <DialogContent v-if="request" class="sm:max-w-lg" size-key="renameRemoteBranch">
      <DialogHeader>
        <DialogTitle>Rename remote branch</DialogTitle>
        <DialogDescription>
          <span class="font-mono">{{ request.branch }}</span> gets a new name on {{ request.remote }}, for everyone who uses that remote, not only
          in this repository. It keeps its commits.
        </DialogDescription>
      </DialogHeader>

      <form class="grid gap-3" @submit.prevent="submit">
        <div class="grid gap-1.5">
          <Label for="rename-remote-branch-name" class="text-base">New name on {{ request.remote }}</Label>
          <Input
            id="rename-remote-branch-name"
            v-model="name"
            class="font-mono text-base"
            autocomplete="off"
            spellcheck="false"
            :disabled="submitting"
            :aria-invalid="nameProblem !== null || undefined"
          />
          <p v-if="nameProblem" class="text-xs text-destructive">{{ nameProblem }}</p>
        </div>

        <div class="grid gap-1.5 text-xs text-muted-foreground">
          <p>
            One push creates the new name and deletes <span class="font-mono">{{ request.branch }}</span> together, so the branch is never left
            under both names or neither. Other clones keep <span class="font-mono">{{ short }}</span> until they fetch, and their local branches
            that track it then track nothing until they are pointed at the new name.
          </p>
          <p v-if="trackedBy.length > 0" data-rename-remote-followers>
            Here, <span class="font-mono">{{ listNames(trackedBy) }}</span> {{ trackedBy.length === 1 ? "tracks" : "track" }} it and will track the
            new name afterwards.
          </p>
          <p>
            The remote's default branch cannot be renamed this way: rename it in the repository's settings on the server. A pull request from this
            branch is likely to be closed by the server when the old name goes.
          </p>
        </div>

        <p v-if="error" class="rounded-md border border-destructive/40 bg-destructive/10 px-2.5 py-1.5 text-base break-words text-destructive">
          {{ error }}
        </p>
      </form>

      <DialogFooter>
        <Button type="button" variant="outline" :disabled="submitting" @click="dialogs.cancel()">Cancel</Button>
        <Button type="button" :disabled="!canSubmit" @click="submit">
          <LoaderCircleIcon v-if="submitting" class="animate-spin" />
          Rename
        </Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
</template>
