<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { LoaderCircleIcon } from "@lucide/vue";
import { commands, type OpResult } from "@/lib/bindings";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { errorMessage } from "@/lib/ipc";
import { validateRefName } from "@/lib/refNames";
import { opFailureText, runOp } from "@/lib/repoActions";
import { useDialogsStore } from "@/stores/dialogs";
import { useSessionsStore } from "@/stores/sessions";

/**
 * Add a remote, change its URL (and a separate push URL), rename it, or remove it from this
 * repository. Opened for one remote from the Branches panel, or from Repository › Remotes, where
 * the remote is picked here. The dialog runs the command itself; adding can also ask the caller
 * to fetch the new remote.
 */
const dialogs = useDialogsStore();
const sessions = useSessionsStore();

const request = computed(() => dialogs.current("remote"));
const mode = computed(() => request.value?.mode ?? "add");

/** The remote being edited, renamed or removed. */
const picked = ref("");
const name = ref("");
const url = ref("");
const separatePush = ref(false);
const pushUrl = ref("");
const fetchNow = ref(true);
const error = ref<string | null>(null);
const submitting = ref(false);

const remote = computed(() => request.value?.remotes.find((r) => r.name === picked.value) ?? null);

/** Fills the fields from remote `n`. */
function load(n: string): void {
  picked.value = n;
  const r = request.value?.remotes.find((x) => x.name === n) ?? null;
  url.value = r?.url ?? "";
  separatePush.value = !!r?.pushUrl;
  pushUrl.value = r?.pushUrl ?? "";
  name.value = n;
}

watch(request, (r) => {
  if (!r) return;
  error.value = null;
  submitting.value = false;
  fetchNow.value = true;
  if (r.mode === "add") {
    picked.value = "";
    name.value = r.remotes.some((x) => x.name === "origin") ? "" : "origin";
    url.value = "";
    separatePush.value = false;
    pushUrl.value = "";
  } else {
    load(r.name ?? r.remotes[0]?.name ?? "");
  }
});
watch(picked, (n) => {
  if (request.value && mode.value !== "add" && n !== name.value) load(n);
});

const trimmedName = computed(() => name.value.trim());
const nameProblem = computed(() => {
  if (mode.value !== "add" && mode.value !== "rename") return null;
  if (trimmedName.value === "") return null;
  if (request.value?.remotes.some((r) => r.name === trimmedName.value && r.name !== picked.value)) return `There is already a remote ${trimmedName.value}.`;
  return validateRefName(trimmedName.value, "remote");
});
function urlProblem(text: string): string | null {
  if (text.trim() === "") return null;
  return text.trim().startsWith("-") ? 'A URL cannot start with "-".' : null;
}

/** For Remove: what goes with the remote, from the refs as last loaded. */
const removal = computed(() => {
  const refs = request.value ? (sessions.sessions.get(request.value.key)?.refs?.refs ?? []) : [];
  const n = picked.value;
  return {
    remoteBranches: refs.filter((r) => r.kind.type === "remoteBranch" && r.kind.remote === n).length,
    tracking: refs.filter((r) => r.kind.type === "localBranch" && r.upstream?.startsWith(`refs/remotes/${n}/`)).map((r) => r.short),
  };
});

const canSubmit = computed(() => {
  if (!request.value || submitting.value) return false;
  switch (mode.value) {
    case "add":
      return trimmedName.value !== "" && nameProblem.value === null && url.value.trim() !== "" && urlProblem(url.value) === null;
    case "edit": {
      const pushOk = !separatePush.value || (pushUrl.value.trim() !== "" && urlProblem(pushUrl.value) === null);
      const changed =
        url.value.trim() !== (remote.value?.url ?? "") || (separatePush.value ? pushUrl.value.trim() : null) !== (remote.value?.pushUrl ?? null);
      return remote.value !== null && url.value.trim() !== "" && urlProblem(url.value) === null && pushOk && changed;
    }
    case "rename":
      return remote.value !== null && trimmedName.value !== "" && trimmedName.value !== picked.value && nameProblem.value === null;
    case "remove":
      return remote.value !== null;
  }
  return false;
});

const titles = { add: "Add remote", edit: "Edit remote", rename: "Rename remote", remove: "Remove remote" };
const buttons = { add: "Add Remote", edit: "Save", rename: "Rename", remove: "Remove Remote" };

function onOpenChange(open: boolean): void {
  if (!open) dialogs.cancel();
}

async function submit(): Promise<void> {
  const r = request.value;
  if (!r || !canSubmit.value) return;
  const key = r.key;
  const n = trimmedName.value;
  const from = picked.value;
  submitting.value = true;
  error.value = null;
  try {
    let result: OpResult;
    let done: string;
    switch (r.mode) {
      case "add":
        result = await runOp(key, () => commands.opRemoteAdd(key, n, url.value.trim()));
        done = `Added remote ${n}.`;
        break;
      case "edit":
        result = await runOp(key, () => commands.opRemoteSetUrl(key, from, url.value.trim(), separatePush.value ? pushUrl.value.trim() : null));
        done = `Updated remote ${from}.`;
        break;
      case "rename":
        result = await runOp(key, () => commands.opRemoteRename(key, from, n));
        done = `Renamed remote ${from} to ${n}.`;
        break;
      case "remove":
        result = await runOp(key, () => commands.opRemoteRemove(key, from));
        done = `Removed remote ${from} from this repository.`;
        break;
    }
    if (!result.ok) {
      error.value = opFailureText(result, titles[r.mode]);
      return;
    }
    // Adding with Fetch ticked: the caller fetches and says how that went.
    const fetching = r.mode === "add" && fetchNow.value;
    if (!fetching) sessions.notifySuccess(done);
    dialogs.resolve("remote", { name: r.mode === "add" || r.mode === "rename" ? n : from, fetch: fetching });
  } catch (e) {
    error.value = errorMessage(e);
  } finally {
    submitting.value = false;
  }
}

const selectClass =
  "well h-8 w-full rounded-lg border px-2.5 font-mono text-base outline-none focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/50";
</script>

<template>
  <Dialog :open="request !== null" @update:open="onOpenChange">
    <DialogContent v-if="request" class="sm:max-w-lg" size-key="remote">
      <DialogHeader>
        <DialogTitle>{{ titles[mode] }}</DialogTitle>
        <DialogDescription v-if="mode === 'add'">
          A remote is another copy of this repository to fetch from and push to, usually on a server.
        </DialogDescription>
        <DialogDescription v-else-if="mode === 'rename'">
          Its remote-tracking branches and the branches that track it follow the new name. The repository on the server is not changed.
        </DialogDescription>
        <DialogDescription v-else-if="mode === 'remove'">
          Only this repository forgets the remote. The repository on the server, and its branches, are not touched.
        </DialogDescription>
        <DialogDescription v-else>Where this repository fetches from and pushes to.</DialogDescription>
      </DialogHeader>

      <form class="grid gap-3" @submit.prevent="submit">
        <div v-if="mode !== 'add'" class="grid gap-1.5">
          <Label for="remote-pick" class="text-base">Remote</Label>
          <select id="remote-pick" v-model="picked" :class="selectClass" :disabled="request.name !== null || submitting">
            <option v-for="r in request.remotes" :key="r.name" :value="r.name">{{ r.name }}</option>
          </select>
          <p v-if="mode !== 'edit' && remote?.url" class="truncate font-mono text-xs text-muted-foreground" :title="remote.url">{{ remote.url }}</p>
        </div>

        <div v-if="mode === 'add' || mode === 'rename'" class="grid gap-1.5">
          <Label for="remote-name" class="text-base">{{ mode === "rename" ? "New name" : "Name" }}</Label>
          <Input
            id="remote-name"
            v-model="name"
            class="font-mono text-base"
            :placeholder="mode === 'add' ? 'upstream' : undefined"
            autocomplete="off"
            spellcheck="false"
            :aria-invalid="nameProblem !== null || undefined"
          />
          <p v-if="nameProblem" class="text-xs text-destructive">{{ nameProblem }}</p>
        </div>

        <template v-if="mode === 'add' || mode === 'edit'">
          <div class="grid gap-1.5">
            <Label for="remote-url" class="text-base">URL</Label>
            <Input
              id="remote-url"
              v-model="url"
              class="font-mono text-base"
              placeholder="git@github.com:owner/repo.git"
              autocomplete="off"
              spellcheck="false"
              :aria-invalid="urlProblem(url) !== null || undefined"
            />
            <p v-if="urlProblem(url)" class="text-xs text-destructive">{{ urlProblem(url) }}</p>
          </div>
          <div class="grid gap-1.5">
            <label class="flex items-center gap-2 text-base select-none">
              <input v-model="separatePush" type="checkbox" class="size-3.5 accent-primary" />
              Push to a different URL
            </label>
            <template v-if="separatePush">
              <Input
                v-model="pushUrl"
                class="font-mono text-base"
                aria-label="Push URL"
                autocomplete="off"
                spellcheck="false"
                :aria-invalid="urlProblem(pushUrl) !== null || undefined"
              />
              <p v-if="urlProblem(pushUrl)" class="text-xs text-destructive">{{ urlProblem(pushUrl) }}</p>
            </template>
          </div>
          <label v-if="mode === 'add'" class="flex items-center gap-2 text-base select-none">
            <input v-model="fetchNow" type="checkbox" class="size-3.5 accent-primary" />
            Fetch it now, so its branches appear
          </label>
        </template>

        <div v-if="mode === 'remove' && remote" class="grid gap-1 text-base">
          <p>
            {{
              removal.remoteBranches === 0
                ? "There are no remote-tracking branches of it here."
                : `Its ${removal.remoteBranches === 1 ? "remote-tracking branch is" : `${removal.remoteBranches} remote-tracking branches are`} deleted from this repository.`
            }}
          </p>
          <p v-if="removal.tracking.length > 0">
            {{ removal.tracking.length === 1 ? "This branch tracks" : "These branches track" }} it, and will track nothing:
            <span class="font-mono">{{ removal.tracking.join(", ") }}</span>.
          </p>
        </div>

        <p v-if="error" class="rounded-md border border-destructive/40 bg-destructive/10 px-2.5 py-1.5 text-base break-words text-destructive">
          {{ error }}
        </p>
      </form>

      <DialogFooter>
        <Button type="button" variant="outline" @click="dialogs.cancel()">Cancel</Button>
        <Button type="button" :variant="mode === 'remove' ? 'destructive' : 'default'" :disabled="!canSubmit" data-remote-submit @click="submit">
          <LoaderCircleIcon v-if="submitting" class="animate-spin" />
          {{ buttons[mode] }}
        </Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
</template>
