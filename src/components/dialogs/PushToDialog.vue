<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { commands, type DroppedCommits } from "@/lib/bindings";
import { formatRelativeTime, shortOid } from "@/lib/format";
import { errorMessage, unwrap } from "@/lib/ipc";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Label } from "@/components/ui/label";
import { useDialogsStore } from "@/stores/dialogs";

/**
 * Push To: push a local branch to a branch of the user's choosing, on one of the remotes or in
 * this repository itself. Only a fast-forward goes through unless Force push is ticked. The caller
 * runs the push.
 */
const dialogs = useDialogsStore();

const request = computed(() => dialogs.current("pushTo"));

const serverIndex = ref(0);
const target = ref("");
const force = ref(false);

const server = computed(() => request.value?.servers[serverIndex.value] ?? null);
const branches = computed(() => server.value?.branches ?? []);
const names = computed(() => branches.value.map((b) => b.name));
/** The commit shown for the chosen branch: what the dropped list is worked out from, and what a force push's lease expects. */
const tip = computed(() => branches.value.find((b) => b.name === target.value)?.oid ?? null);

/**
 * With Force push ticked, the commits it would drop from the chosen branch, listed before the user
 * confirms; null while they load. `droppedError` says why they could not be listed, which blocks
 * the force push.
 */
const dropped = ref<DroppedCommits | null>(null);
const droppedError = ref<string | null>(null);
watch(
  () => [request.value, force.value, tip.value] as const,
  async ([r, forcing, at]) => {
    dropped.value = null;
    droppedError.value = null;
    if (!r || !forcing || at === null) return;
    try {
      const list = await unwrap(commands.getCommitsDropped(r.key, r.sourceOid, at));
      if (request.value === r && force.value && tip.value === at) dropped.value = list;
    } catch (e) {
      if (request.value === r) droppedError.value = errorMessage(e);
    }
  },
);

watch(request, (r) => {
  if (!r) return;
  serverIndex.value = r.server;
  const offered = (r.servers[r.server]?.branches ?? []).map((b) => b.name);
  target.value = r.target !== null && offered.includes(r.target) ? r.target : (offered[0] ?? "");
  force.value = r.force;
});

/** Another server lists other branches: keep the same name when it has one, else take its first. */
watch(serverIndex, () => {
  if (!names.value.includes(target.value)) target.value = names.value[0] ?? "";
});

/** Where it goes, as the text says it: `origin/dev`, or just `dev` in this repository. */
const destination = computed(() => (server.value?.remote ? `${server.value.remote}/${target.value}` : target.value));
/** A force push waits for the list of what it drops, so nothing is dropped unseen. */
const canSubmit = computed(() => request.value !== null && server.value !== null && tip.value !== null && (!force.value || dropped.value !== null));

const selectClass =
  "well h-8 w-full rounded-lg border px-2.5 text-base outline-none focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/50";

function onOpenChange(open: boolean): void {
  if (!open) dialogs.cancel();
}

function submit(): void {
  if (!canSubmit.value || !server.value || tip.value === null) return;
  dialogs.resolve("pushTo", { remote: server.value.remote, target: target.value, force: force.value, expected: tip.value, source: request.value!.sourceOid });
}
</script>

<template>
  <Dialog :open="request !== null" @update:open="onOpenChange">
    <DialogContent v-if="request" class="sm:max-w-lg" size-key="pushTo">
      <DialogHeader>
        <DialogTitle>Push {{ request.branch }} To</DialogTitle>
        <DialogDescription>
          Moves the chosen branch to <span class="font-mono">{{ request.branch }}</span>'s latest commit. Nothing is checked out, so your
          files stay as they are.
        </DialogDescription>
      </DialogHeader>

      <form class="grid gap-3" @submit.prevent="submit">
        <div class="grid gap-1.5">
          <Label for="push-to-server" class="text-base">Server</Label>
          <select id="push-to-server" v-model.number="serverIndex" :class="selectClass">
            <option v-for="(s, i) in request.servers" :key="s.label" :value="i">{{ s.label }}</option>
          </select>
        </div>
        <div class="grid gap-1.5">
          <Label for="push-to-branch" class="text-base">Branch</Label>
          <select id="push-to-branch" v-model="target" :class="selectClass" class="font-mono" :disabled="branches.length === 0">
            <option v-for="b in names" :key="b" :value="b">{{ b }}</option>
          </select>
          <p v-if="branches.length === 0" class="text-xs text-modified-fg">
            {{ server?.remote ? `No branches of ${server.remote} have been fetched. Fetch first.` : "There is no other branch here to push to." }}
          </p>
          <p v-else-if="server && server.remote === null" class="text-xs text-muted-foreground">
            The checked-out branch is not listed: Git cannot move it without changing your files. Use Merge or Pull for it.
          </p>
        </div>
        <div class="grid gap-1">
          <label class="flex items-center gap-2 text-base select-none">
            <input v-model="force" type="checkbox" class="size-3.5 accent-destructive" data-push-to-force />
            Force push
          </label>
          <template v-if="force">
            <p v-if="droppedError" class="text-xs text-destructive">Could not list what the force push would drop: {{ droppedError }}</p>
            <p v-else-if="dropped === null" class="text-xs text-muted-foreground">Listing what the force push would drop…</p>
            <p v-else-if="dropped.total === 0" class="text-xs text-muted-foreground">
              Nothing is dropped: <span class="font-mono">{{ request.branch }}</span> already has every commit on
              <span class="font-mono">{{ destination }}</span>, so this is a fast-forward and needs no force.
            </p>
            <div v-else class="grid gap-1" data-push-to-dropped>
              <p class="text-xs text-destructive">
                {{ dropped.total === 1 ? "This commit is" : `These ${dropped.total} commits are` }} dropped from
                <span class="font-mono">{{ destination }}</span>{{ server?.remote ? ", for everyone who uses it" : "" }}:
              </p>
              <ul class="max-h-36 overflow-auto rounded border border-destructive/40 bg-destructive/5 px-2 py-1 text-xs scrollbar-thin">
                <li v-for="c in dropped.commits" :key="c.oid" class="flex gap-2 truncate leading-5">
                  <span class="shrink-0 font-mono text-muted-foreground">{{ shortOid(c.oid) }}</span>
                  <span class="min-w-0 truncate" :title="c.subject">{{ c.subject }}</span>
                  <span class="ml-auto shrink-0 text-muted-foreground">{{ c.authorName }}, {{ formatRelativeTime(c.authorTime) }}</span>
                </li>
                <li v-if="dropped.total > dropped.commits.length" class="leading-5 text-muted-foreground">
                  and {{ dropped.total - dropped.commits.length }} more
                </li>
              </ul>
              <p class="text-xs text-muted-foreground">
                If <span class="font-mono">{{ destination }}</span> has moved on from the commit this list is for
                (<span class="font-mono">{{ tip ? shortOid(tip) : "" }}</span>), or <span class="font-mono">{{ request.branch }}</span> from
                <span class="font-mono">{{ shortOid(request.sourceOid) }}</span>, the push is refused, so nothing else is lost.
              </p>
            </div>
          </template>
          <p v-else class="text-xs text-muted-foreground">
            Without it, Git only moves <span class="font-mono">{{ destination || "the branch" }}</span> forward, and refuses when it has commits that
            <span class="font-mono">{{ request.branch }}</span> does not.
          </p>
        </div>
      </form>

      <DialogFooter>
        <Button type="button" variant="outline" @click="dialogs.cancel()">Cancel</Button>
        <Button type="button" :variant="force ? 'destructive' : 'default'" :disabled="!canSubmit" data-push-to-submit @click="submit">
          {{ force ? "Force Push" : "Push" }}
        </Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
</template>
