<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { LoaderCircleIcon } from "@lucide/vue";
import { commands } from "@/lib/bindings";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Label } from "@/components/ui/label";
import { errorMessage } from "@/lib/ipc";
import { rebaseOutcome, runOp } from "@/lib/repoActions";
import { useDialogsStore } from "@/stores/dialogs";
import { useSessionsStore } from "@/stores/sessions";

const dialogs = useDialogsStore();
const sessions = useSessionsStore();

const request = computed(() => dialogs.current("rebase"));

const chosen = ref("");
const error = ref<string | null>(null);
const submitting = ref(false);

watch(request, (r) => {
  if (!r) return;
  const upstream = r.upstream === null ? undefined : r.branches.find((b) => b.short === r.upstream);
  chosen.value = upstream?.short ?? r.branches[0]?.short ?? "";
  error.value = null;
  submitting.value = false;
});

const local = computed(() => request.value?.branches.filter((b) => !b.remote) ?? []);
const remote = computed(() => request.value?.branches.filter((b) => b.remote) ?? []);
const canSubmit = computed(() => chosen.value !== "" && !submitting.value);

const selectClass =
  "well h-8 w-full rounded-lg border px-2.5 text-base outline-none focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/50";

function onOpenChange(open: boolean): void {
  if (!open) dialogs.cancel();
}

async function submit(): Promise<void> {
  const r = request.value;
  if (!r || !canSubmit.value) return;
  const onto = chosen.value;
  submitting.value = true;
  error.value = null;
  try {
    const revision = r.branches.find((b) => b.short === onto)?.revision ?? onto;
    const result = await runOp(r.key, () => commands.opRebase(r.key, revision, r.branch, r.autostash));
    let outcome = rebaseOutcome(result, r.branch, onto, false, r.autostash);
    if (outcome.failed) {
      // Git can stop part way without a conflict (an untracked file in the way): only the
      // status says whether the rebase is still open.
      await sessions.refetchRefsAndStatus(r.key);
      if (sessions.sessions.get(r.key)?.status?.inProgress?.kind === "rebase") outcome = rebaseOutcome(result, r.branch, onto, true, r.autostash);
    }
    // A plain failure leaves nothing open, so the dialog stays for another choice.
    if (outcome.failed) {
      error.value = outcome.message;
      return;
    }
    if (outcome.kind === "success") sessions.notifySuccess(outcome.message);
    else sessions.notify(outcome.message);
    dialogs.resolve("rebase", result);
  } catch (e) {
    error.value = errorMessage(e);
  } finally {
    submitting.value = false;
  }
}
</script>

<template>
  <Dialog :open="request !== null" @update:open="onOpenChange">
    <DialogContent v-if="request" class="sm:max-w-lg" size-key="rebase">
      <DialogHeader>
        <DialogTitle>Rebase {{ request.branch }}</DialogTitle>
        <DialogDescription>
          Replays the commits on <span class="font-mono">{{ request.branch }}</span> that the branch you choose does not have on top of
          it, one at a time, so the history stays in one line (git rebase). If a commit conflicts, the rebase stops on it so you can
          resolve it.
        </DialogDescription>
      </DialogHeader>

      <form class="grid gap-3" @submit.prevent="submit">
        <div class="grid gap-1.5">
          <Label for="rebase-onto" class="text-base">Rebase onto</Label>
          <select id="rebase-onto" v-model="chosen" :class="selectClass" :disabled="submitting">
            <optgroup v-if="local.length > 0" label="Local branches">
              <option v-for="b in local" :key="b.short" :value="b.short">{{ b.short }}</option>
            </optgroup>
            <optgroup v-if="remote.length > 0" label="Remote branches">
              <option v-for="b in remote" :key="b.short" :value="b.short">
                {{ b.short }}{{ b.short === request.upstream ? ` (tracked by ${request.branch})` : "" }}
              </option>
            </optgroup>
          </select>
        </div>

        <p class="text-xs text-muted-foreground">
          The replayed commits get new ids. If you have already pushed {{ request.branch }} and others work on it, pushing after a rebase
          needs a force push, which StashPanda does not do.
        </p>

        <p v-if="request.autostash" class="text-xs text-muted-foreground">
          Your uncommitted changes are stashed first and put back after the rebase.
        </p>

        <p v-if="error" class="rounded-md border border-destructive/40 bg-destructive/10 px-2.5 py-1.5 text-base break-words text-destructive">
          {{ error }}
        </p>
      </form>

      <DialogFooter>
        <Button type="button" variant="outline" @click="dialogs.cancel()">Cancel</Button>
        <Button type="button" :disabled="!canSubmit" @click="submit">
          <LoaderCircleIcon v-if="submitting" class="animate-spin" />
          Rebase
        </Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
</template>
