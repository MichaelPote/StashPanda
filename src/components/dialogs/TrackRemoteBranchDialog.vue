<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { LoaderCircleIcon } from "@lucide/vue";
import { commands } from "@/lib/bindings";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Label } from "@/components/ui/label";
import { errorMessage } from "@/lib/ipc";
import { opFailureText, runOp } from "@/lib/repoActions";
import { useDialogsStore, type TrackChoice } from "@/stores/dialogs";
import { useSessionsStore } from "@/stores/sessions";

/**
 * Chooses the remote branch a local branch tracks (its upstream), or stops it tracking one. Runs
 * `git branch --set-upstream-to` or `git branch --unset-upstream` itself, and stays open with
 * Git's message when that fails.
 */
const dialogs = useDialogsStore();
const sessions = useSessionsStore();

const request = computed(() => dialogs.current("trackRemoteBranch"));

/** The full name of the chosen remote-tracking branch, or "" for no remote branch. */
const chosen = ref("");
const error = ref<string | null>(null);
const submitting = ref(false);

watch(request, (r) => {
  if (!r) return;
  chosen.value = r.upstream ?? "";
  error.value = null;
  submitting.value = false;
});

/** `refs/remotes/origin/dev` as `origin/dev`, and `refs/heads/main` as `main`. */
function shortName(fullName: string): string {
  return fullName.replace(/^refs\/(remotes|heads)\//, "");
}

/** The remote branches, grouped by remote in the order the refs list gives them. */
const groups = computed(() => {
  const byRemote = new Map<string, TrackChoice[]>();
  for (const c of request.value?.choices ?? []) {
    const list = byRemote.get(c.remote);
    if (list) list.push(c);
    else byRemote.set(c.remote, [c]);
  }
  return [...byRemote.entries()].map(([remote, choices]) => ({ remote, choices }));
});

/**
 * What the branch tracks now when that is not one of the remote branches listed: a remote branch
 * that has gone since the last fetch, or a local branch. Offered first, so saving without a
 * change keeps it.
 */
const unlistedUpstream = computed(() => {
  const r = request.value;
  if (!r || r.upstream === null || r.choices.some((c) => c.fullName === r.upstream)) return null;
  const note = r.upstream.startsWith("refs/heads/") ? "a local branch" : "no longer on the remote";
  return { fullName: r.upstream, label: `${shortName(r.upstream)} (${note})` };
});

const currentText = computed(() => {
  const r = request.value;
  if (!r) return "";
  return r.upstream === null ? `${r.branch} tracks no remote branch now.` : `${r.branch} tracks ${shortName(r.upstream)} now.`;
});

const selectClass =
  "well h-8 w-full rounded-lg border px-2.5 text-base outline-none focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/50";

function onOpenChange(open: boolean): void {
  if (!open) dialogs.cancel();
}

async function submit(): Promise<void> {
  const r = request.value;
  if (!r || submitting.value) return;
  const upstream = chosen.value === "" ? null : chosen.value;
  // Nothing changed: Git would only repeat the setting, or fail to unset an upstream that is not there.
  if (upstream === r.upstream) {
    dialogs.resolve("trackRemoteBranch", null);
    return;
  }
  submitting.value = true;
  error.value = null;
  try {
    const result = await runOp(r.key, () => commands.opBranchSetUpstream(r.key, r.branch, upstream));
    if (!result.ok) {
      error.value = opFailureText(result, `Changing what ${r.branch} tracks`);
      return;
    }
    sessions.notifySuccess(upstream === null ? `${r.branch} no longer tracks a remote branch.` : `${r.branch} now tracks ${shortName(upstream)}.`);
    dialogs.resolve("trackRemoteBranch", result);
  } catch (e) {
    error.value = errorMessage(e);
  } finally {
    submitting.value = false;
  }
}
</script>

<template>
  <Dialog :open="request !== null" @update:open="onOpenChange">
    <DialogContent v-if="request" class="sm:max-w-lg" size-key="trackRemoteBranch">
      <DialogHeader>
        <DialogTitle>Track Remote Branch</DialogTitle>
        <DialogDescription>
          Choose the remote branch that <span class="font-mono">{{ request.branch }}</span> tracks (its upstream). Pull, Push and Sync use
          it without asking, and the branch shows how many commits it is ahead of it and behind it.
        </DialogDescription>
      </DialogHeader>

      <form class="grid gap-3" @submit.prevent="submit">
        <div class="grid gap-1.5">
          <Label for="track-upstream" class="text-base">Remote branch</Label>
          <select id="track-upstream" v-model="chosen" :class="selectClass" :disabled="submitting">
            <option value="">Don't track a remote branch</option>
            <option v-if="unlistedUpstream" :value="unlistedUpstream.fullName">{{ unlistedUpstream.label }}</option>
            <optgroup v-for="g in groups" :key="g.remote" :label="g.remote">
              <option v-for="c in g.choices" :key="c.fullName" :value="c.fullName">{{ c.short }}</option>
            </optgroup>
          </select>
          <p class="text-xs text-muted-foreground">{{ currentText }} Changing it changes nothing on the remote.</p>
        </div>

        <p v-if="error" class="rounded-md border border-destructive/40 bg-destructive/10 px-2.5 py-1.5 text-base break-words text-destructive">
          {{ error }}
        </p>
      </form>

      <DialogFooter>
        <Button type="button" variant="outline" @click="dialogs.cancel()">Cancel</Button>
        <Button type="button" :disabled="submitting" @click="submit">
          <LoaderCircleIcon v-if="submitting" class="animate-spin" />
          Save
        </Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
</template>
