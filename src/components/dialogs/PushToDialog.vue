<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { commands, type DroppedCommits } from "@/lib/bindings";
import { formatRelativeTime, shortOid } from "@/lib/format";
import { errorMessage, unwrap } from "@/lib/ipc";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { validateRefName } from "@/lib/refNames";
import { useDialogsStore } from "@/stores/dialogs";

/**
 * Push To: push a local branch to a branch of the user's choosing, on one of the remotes or in
 * this repository itself, or to a new branch on a remote, named in a field of its own. Only a
 * fast-forward goes through unless Force push is ticked; a new branch is never forced. A push to
 * a remote can also make the branch track where it went. The caller runs the push.
 */
const dialogs = useDialogsStore();

const request = computed(() => dialogs.current("pushTo"));

/** The Branch list's entry for a new branch: a ":" can never be in a branch name. */
const NEW_BRANCH = ":new";

const serverIndex = ref(0);
/** The branch picked in the Branch list, or `NEW_BRANCH`. */
const target = ref("");
/** The new branch's name, as typed. */
const newName = ref("");
const force = ref(false);
const track = ref(false);

const server = computed(() => request.value?.servers[serverIndex.value] ?? null);
const branches = computed(() => server.value?.branches ?? []);
const names = computed(() => branches.value.map((b) => b.name));
/** Only a remote gets a new branch here; in this repository, Create Branch Here… does that. */
const canCreate = computed(() => server.value?.remote != null);
const typing = computed(() => canCreate.value && target.value === NEW_BRANCH);
/** The name of the branch pushed to, picked or typed. */
const targetName = computed(() => (typing.value ? newName.value.trim() : target.value));
/** The commit shown for the chosen branch: what the dropped list is worked out from, and what a force push's lease expects. */
const tip = computed(() => branches.value.find((b) => b.name === targetName.value)?.oid ?? null);
/** A name typed that the remote has no branch of, as last fetched: the push creates it. */
const creating = computed(() => typing.value && tip.value === null);
const nameProblem = computed(() => (typing.value ? validateRefName(targetName.value, "branch") : null));
/** Tracking is offered for a remote branch, unless the branch tracks it already. */
const canTrack = computed(
  () => server.value?.remote != null && targetName.value !== "" && request.value?.upstream !== `refs/remotes/${server.value.remote}/${targetName.value}`,
);

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

/**
 * Picks branch `name` when the server has it; on a remote that does not, the new branch, named
 * `name`; in this repository, its first branch.
 */
function pick(name: string): void {
  if (names.value.includes(name)) target.value = name;
  else if (canCreate.value) {
    target.value = NEW_BRANCH;
    newName.value = name;
  } else target.value = names.value[0] ?? "";
}

watch(request, (r) => {
  if (!r) return;
  serverIndex.value = r.server;
  newName.value = r.branch;
  pick(r.target ?? r.branch);
  force.value = r.force;
  track.value = creating.value;
});

/** Another server lists other branches: keep the same name there. */
watch(serverIndex, () => pick(targetName.value || (request.value?.branch ?? "")));

/**
 * A new branch has nothing to drop, so there is nothing to force; and it is usually where the
 * branch is to be pushed from now on, so it is tracked unless the box is cleared.
 */
watch(creating, (c) => {
  if (c) force.value = false;
  track.value = c;
});

/** Where it goes, as the text says it: `origin/dev`, or just `dev` in this repository. */
const destination = computed(() => (server.value?.remote ? `${server.value.remote}/${targetName.value}` : targetName.value));
/** A force push waits for the list of what it drops, so nothing is dropped unseen. */
const canSubmit = computed(() => {
  if (request.value === null || server.value === null) return false;
  if (creating.value) return nameProblem.value === null;
  return tip.value !== null && (!force.value || dropped.value !== null);
});

const selectClass =
  "well h-8 w-full rounded-lg border px-2.5 text-base outline-none focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/50";

function onOpenChange(open: boolean): void {
  if (!open) dialogs.cancel();
}

function submit(): void {
  if (!canSubmit.value || !server.value) return;
  dialogs.resolve("pushTo", {
    remote: server.value.remote,
    target: targetName.value,
    force: force.value && !creating.value,
    expected: tip.value,
    source: request.value!.sourceOid,
    track: track.value && canTrack.value,
  });
}
</script>

<template>
  <Dialog :open="request !== null" @update:open="onOpenChange">
    <DialogContent v-if="request" class="sm:max-w-lg" size-key="pushTo">
      <DialogHeader>
        <DialogTitle>Push {{ request.branch }} To</DialogTitle>
        <DialogDescription>
          {{ creating ? "Creates the new branch at" : "Moves the chosen branch to" }} <span class="font-mono">{{ request.branch }}</span>'s
          latest commit. Nothing is checked out, so your files stay as they are.
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
          <select id="push-to-branch" v-model="target" :class="[selectClass, !typing && 'font-mono']" :disabled="branches.length === 0 && !canCreate">
            <option v-for="b in names" :key="b" :value="b" class="font-mono">{{ b }}</option>
            <option v-if="canCreate" :value="NEW_BRANCH" class="font-sans">New branch…</option>
          </select>
          <template v-if="typing">
            <Label for="push-to-new-branch" class="mt-1.5 text-base">Name of the new branch on {{ server?.remote }}</Label>
            <Input
              id="push-to-new-branch"
              v-model="newName"
              class="font-mono text-base"
              autocomplete="off"
              spellcheck="false"
              :aria-invalid="nameProblem !== null || undefined"
              data-push-to-new-name
            />
            <p v-if="nameProblem" class="text-xs text-destructive">{{ nameProblem }}</p>
            <p v-else-if="!creating" class="text-xs text-modified-fg">
              {{ server?.remote }} already has a branch {{ targetName }}, so this pushes onto it rather than creating one.
            </p>
            <p v-else class="text-xs text-muted-foreground">
              {{ server?.remote }} has no branch {{ targetName }} as last fetched, so the push creates it.
            </p>
          </template>
          <p v-else-if="branches.length === 0 && !canCreate" class="text-xs text-modified-fg">There is no other branch here to push to.</p>
          <p v-else-if="server && server.remote === null" class="text-xs text-muted-foreground">
            The checked-out branch is not listed: Git cannot move it without changing your files. Use Merge or Pull for it.
          </p>
        </div>
        <div v-if="canTrack" class="grid gap-1">
          <label class="flex items-center gap-2 text-base select-none">
            <input v-model="track" type="checkbox" class="size-3.5 accent-primary" data-push-to-track />
            Track it from now on
          </label>
          <p class="text-xs text-muted-foreground">
            Sets <span class="font-mono">{{ destination }}</span> as the upstream of <span class="font-mono">{{ request.branch }}</span> once
            the push has worked, so Pull and Push use it without asking.
          </p>
        </div>
        <div class="grid gap-1">
          <label class="flex items-center gap-2 text-base select-none">
            <input v-model="force" type="checkbox" class="size-3.5 accent-destructive" :disabled="creating" data-push-to-force />
            Force push
          </label>
          <p v-if="creating" class="text-xs text-muted-foreground">A new branch has no commits to drop, so it needs no force.</p>
          <template v-else-if="force">
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
