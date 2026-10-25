<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { LoaderCircleIcon } from "@lucide/vue";
import { commands } from "@/lib/bindings";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { errorMessage } from "@/lib/ipc";
import { validateRefName } from "@/lib/refNames";
import { opFailureText, runOp } from "@/lib/repoActions";
import { useDialogsStore } from "@/stores/dialogs";
import { useSessionsStore } from "@/stores/sessions";

const dialogs = useDialogsStore();
const sessions = useSessionsStore();

const request = computed(() => dialogs.current("renameBranch"));

const name = ref("");
const renameRemote = ref(false);
const error = ref<string | null>(null);
const submitting = ref(false);

watch(request, (r) => {
  if (!r) return;
  name.value = r.name;
  renameRemote.value = false;
  error.value = null;
  submitting.value = false;
});

/** The remote copy as `origin/main`, for the text. */
const remoteShort = computed(() => (request.value?.remoteCopy ? `${request.value.remoteCopy.remote}/${request.value.remoteCopy.branch}` : ""));

const trimmed = computed(() => name.value.trim());
const nameProblem = computed(() => (trimmed.value.length === 0 ? null : validateRefName(trimmed.value, "branch")));
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
    const result = await runOp(r.key, () => commands.opBranchRename(r.key, r.name, to));
    if (!result.ok && /branch is renamed, but update of config-file failed/.test(result.stderr)) {
      // Git renamed the branch, then could not move its settings (its upstream) in .git/config,
      // usually because another Git process holds config.lock.
      sessions.notify(
        `Renamed branch ${r.name} to ${to}, but Git could not move its settings to the new name, so ${to} may not track a remote branch. ` +
          "Another Git process may be saving to the repository's settings file; use Track Remote Branch… to set it again.",
      );
      dialogs.resolve("renameBranch", { name: to, renameRemote: false });
      return;
    }
    if (!result.ok) {
      error.value = opFailureText(result, `Renaming ${r.name} to ${to}`);
      return;
    }
    // The caller goes on to the remote, and says how that went.
    const onRemote = renameRemote.value && r.remoteCopy !== null;
    if (!onRemote) sessions.notifySuccess(`Renamed branch ${r.name} to ${to}.`);
    dialogs.resolve("renameBranch", { name: to, renameRemote: onRemote });
  } catch (e) {
    error.value = errorMessage(e);
  } finally {
    submitting.value = false;
  }
}
</script>

<template>
  <Dialog :open="request !== null" @update:open="onOpenChange">
    <DialogContent v-if="request" class="sm:max-w-lg" size-key="renameBranch">
      <DialogHeader>
        <DialogTitle>Rename branch</DialogTitle>
        <DialogDescription>
          <span class="font-mono">{{ request.name }}</span> keeps its commits and the remote branch it tracks. A copy on a remote keeps
          the old name.
        </DialogDescription>
      </DialogHeader>

      <form class="grid gap-3" @submit.prevent="submit">
        <div class="grid gap-1.5">
          <Label for="rename-branch-name" class="text-base">New name</Label>
          <Input
            id="rename-branch-name"
            v-model="name"
            class="font-mono text-base"
            autocomplete="off"
            spellcheck="false"
            :aria-invalid="nameProblem !== null || undefined"
          />
          <p v-if="nameProblem" class="text-xs text-destructive">{{ nameProblem }}</p>
        </div>

        <div v-if="request.remoteCopy" class="grid gap-1">
          <label class="flex items-center gap-2 text-base select-none">
            <input v-model="renameRemote" type="checkbox" class="size-3.5 accent-primary" :disabled="submitting" data-rename-remote />
            Also rename <span class="font-mono">{{ remoteShort }}</span> on {{ request.remoteCopy.remote }}
          </label>
          <p class="text-xs text-muted-foreground">
            Pushes the branch to {{ request.remoteCopy.remote }} under the new name, makes it track that, then deletes
            <span class="font-mono">{{ request.remoteCopy.branch }}</span> on {{ request.remoteCopy.remote }}, for everyone who uses it. Other
            clones keep <span class="font-mono">{{ remoteShort }}</span> until they fetch. If it is the remote's default branch, change that
            on the server first, or the deletion is refused.
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
          Rename
        </Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
</template>
