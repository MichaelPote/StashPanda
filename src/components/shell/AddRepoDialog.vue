<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { useDebounceFn } from "@vueuse/core";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { ChevronDownIcon, ChevronRightIcon, FolderOpenIcon, LoaderCircleIcon, TriangleAlertIcon } from "@lucide/vue";
import { commands, type Connection, type RepoResolution } from "@/lib/bindings";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "@/components/ui/collapsible";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { transportLabel } from "@/lib/format";
import { errorMessage, unwrap } from "@/lib/ipc";
import { useHostStore } from "@/stores/host";
import { useLibraryStore } from "@/stores/library";
import { useSessionsStore } from "@/stores/sessions";

const open = defineModel<boolean>("open", { default: false });

const host = useHostStore();
const library = useLibraryStore();
const sessions = useSessionsStore();

type ConnectionKind = Connection["type"];

const path = ref("");
const displayName = ref("");
const kind = ref<ConnectionKind>("auto");
const wslDistro = ref("");
const ssh = ref({ host: "", user: "", port: "", identityFile: "", remoteGit: "" });
const advancedOpen = ref(false);

const distros = ref<string[]>([]);
const isWindows = computed(() => host.info?.type === "windows");

const resolution = ref<RepoResolution | null>(null);
const resolving = ref(false);
const resolveError = ref<string | null>(null);
const submitError = ref<string | null>(null);
const submitting = ref(false);

const connection = computed<Connection>(() => {
  switch (kind.value) {
    case "auto":
      return { type: "auto" };
    case "local":
      return { type: "local" };
    case "wsl":
      return { type: "wsl", distro: wslDistro.value.trim() };
    case "ssh": {
      const port = Number.parseInt(ssh.value.port, 10);
      return {
        type: "ssh",
        host: ssh.value.host.trim(),
        user: ssh.value.user.trim() || null,
        port: Number.isFinite(port) && port > 0 ? port : null,
        identityFile: ssh.value.identityFile.trim() || null,
        remoteGit: ssh.value.remoteGit.trim() || null,
      };
    }
  }
});

/** The connection is complete enough to send to `repoResolve`. */
const connectionReady = computed(() => {
  if (kind.value === "wsl") return wslDistro.value.trim().length > 0;
  if (kind.value === "ssh") return ssh.value.host.trim().length > 0;
  return true;
});

let resolveSerial = 0;

async function resolveNow(): Promise<void> {
  const p = path.value.trim();
  if (p.length === 0 || !connectionReady.value) {
    resolution.value = null;
    resolveError.value = null;
    resolving.value = false;
    return;
  }
  const serial = ++resolveSerial;
  resolving.value = true;
  try {
    const result = await unwrap(commands.repoResolve(p, connection.value));
    if (serial !== resolveSerial) return;
    resolution.value = result;
    resolveError.value = null;
  } catch (e) {
    if (serial !== resolveSerial) return;
    resolution.value = null;
    resolveError.value = errorMessage(e);
  } finally {
    if (serial === resolveSerial) resolving.value = false;
  }
}

const resolveDebounced = useDebounceFn(resolveNow, 300);

watch([path, connection], () => {
  submitError.value = null;
  resolving.value = path.value.trim().length > 0 && connectionReady.value;
  void resolveDebounced();
});

watch(open, async (isOpen) => {
  if (!isOpen) return;
  reset();
  if (isWindows.value && distros.value.length === 0) {
    try {
      distros.value = await unwrap(commands.wslListDistros());
      if (distros.value.length > 0 && wslDistro.value === "") wslDistro.value = distros.value[0] as string;
    } catch {
      // No WSL on this Windows machine; the distro field stays free text.
    }
  }
});

function reset(): void {
  path.value = "";
  displayName.value = "";
  kind.value = "auto";
  ssh.value = { host: "", user: "", port: "", identityFile: "", remoteGit: "" };
  resolution.value = null;
  resolveError.value = null;
  submitError.value = null;
  resolving.value = false;
  submitting.value = false;
  advancedOpen.value = false;
  resolveSerial += 1;
}

async function browse(): Promise<void> {
  try {
    const picked = await openDialog({ directory: true, multiple: false, title: "Choose a repository folder" });
    if (typeof picked === "string" && picked.length > 0) path.value = picked;
  } catch (e) {
    submitError.value = errorMessage(e);
  }
}

const canSubmit = computed(
  () => path.value.trim().length > 0 && connectionReady.value && !submitting.value,
);

async function submit(): Promise<void> {
  if (!canSubmit.value) return;
  submitting.value = true;
  submitError.value = null;
  try {
    const entry = await library.addRepo(
      path.value.trim(),
      connection.value,
      null,
      displayName.value.trim() || null,
    );
    if (!entry.key) throw new Error("The backend returned a library entry without a key.");
    await sessions.openTab(entry.key);
    open.value = false;
  } catch (e) {
    submitError.value = errorMessage(e);
  } finally {
    submitting.value = false;
  }
}

const kinds: { id: ConnectionKind; label: string }[] = [
  { id: "auto", label: "Auto (detect from the path)" },
  { id: "local", label: "Local" },
  { id: "wsl", label: "WSL" },
  { id: "ssh", label: "SSH" },
];

const selectClass =
  "h-8 w-full rounded-lg border border-input bg-transparent px-2.5 text-base outline-none focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/50 dark:bg-input/30";
</script>

<template>
  <Dialog v-model:open="open">
    <DialogContent class="sm:max-w-lg">
      <DialogHeader>
        <DialogTitle>Add repository</DialogTitle>
        <DialogDescription>
          Enter a Windows, UNC, Linux or <span class="font-mono">user@host:/path</span> location.
          The library remembers it and opens it in a new tab.
        </DialogDescription>
      </DialogHeader>

      <form class="grid gap-3" @submit.prevent="submit">
        <div class="grid gap-1.5">
          <Label for="add-repo-path" class="text-base">Repository path</Label>
          <div class="flex gap-2">
            <Input
              id="add-repo-path"
              v-model="path"
              class="font-mono text-base"
              placeholder="C:\src\project, \\wsl.localhost\Ubuntu\home\me\project or /home/me/code/project"
              autocomplete="off"
              spellcheck="false"
            />
            <Button type="button" variant="outline" size="sm" class="h-8 shrink-0" @click="browse">
              <FolderOpenIcon />
              Browse…
            </Button>
          </div>
        </div>

        <div
          v-if="path.trim().length > 0"
          class="rounded-lg border border-border bg-muted/30 px-3 py-2 text-base"
        >
          <div v-if="resolving" class="flex items-center gap-2 text-muted-foreground">
            <LoaderCircleIcon class="size-3.5 animate-spin" />
            Resolving…
          </div>
          <p v-else-if="resolveError" class="text-deleted-fg">{{ resolveError }}</p>
          <div v-else-if="resolution" class="grid gap-1.5">
            <div class="flex items-center gap-2">
              <Badge variant="secondary" class="h-5 text-xs">
                {{ transportLabel(resolution.resolvedTransport) }}
              </Badge>
              <span class="truncate font-medium">{{ resolution.displayName }}</span>
            </div>
            <dl class="grid grid-cols-[auto_1fr] gap-x-3 gap-y-0.5 text-xs">
              <dt class="text-muted-foreground">Native path</dt>
              <dd class="truncate font-mono" :title="resolution.nativePath">{{ resolution.nativePath }}</dd>
              <template v-if="resolution.hostPath && resolution.hostPath !== resolution.nativePath">
                <dt class="text-muted-foreground">Host path</dt>
                <dd class="truncate font-mono" :title="resolution.hostPath">{{ resolution.hostPath }}</dd>
              </template>
              <dt class="text-muted-foreground">Key</dt>
              <dd class="truncate font-mono text-muted-foreground" :title="resolution.key">{{ resolution.key }}</dd>
            </dl>
            <ul v-if="resolution.warnings.length > 0" class="mt-1 grid gap-1">
              <li
                v-for="(warning, i) in resolution.warnings"
                :key="i"
                class="flex items-start gap-1.5 text-modified-fg"
              >
                <TriangleAlertIcon class="mt-px size-3.5 shrink-0" />
                <span>{{ warning }}</span>
              </li>
            </ul>
          </div>
          <p v-else class="text-muted-foreground">Fill in the connection details to resolve this path.</p>
        </div>

        <div class="grid gap-1.5">
          <Label for="add-repo-name" class="text-base">Display name <span class="text-muted-foreground">(optional)</span></Label>
          <Input
            id="add-repo-name"
            v-model="displayName"
            class="text-base"
            :placeholder="resolution?.displayName ?? 'Defaults to the folder name'"
            autocomplete="off"
          />
        </div>

        <Collapsible v-model:open="advancedOpen">
          <CollapsibleTrigger as-child>
            <button
              type="button"
              class="flex items-center gap-1 text-base text-muted-foreground hover:text-foreground"
            >
              <component :is="advancedOpen ? ChevronDownIcon : ChevronRightIcon" class="size-3.5" />
              Advanced
            </button>
          </CollapsibleTrigger>
          <CollapsibleContent>
            <div class="mt-2 grid gap-3 rounded-lg border border-border p-3">
              <div class="grid gap-1.5">
                <Label for="add-repo-connection" class="text-base">Connection</Label>
                <select id="add-repo-connection" v-model="kind" :class="selectClass">
                  <option v-for="k in kinds" :key="k.id" :value="k.id">{{ k.label }}</option>
                </select>
              </div>

              <div v-if="kind === 'wsl'" class="grid gap-1.5">
                <Label for="add-repo-distro" class="text-base">WSL distribution</Label>
                <Input
                  id="add-repo-distro"
                  v-model="wslDistro"
                  class="text-base"
                  placeholder="Ubuntu"
                  list="add-repo-distro-list"
                  autocomplete="off"
                />
                <datalist id="add-repo-distro-list">
                  <option v-for="d in distros" :key="d" :value="d" />
                </datalist>
                <p v-if="!isWindows" class="text-xs text-muted-foreground">
                  WSL connections only apply when StashPanda runs on Windows.
                </p>
              </div>

              <div v-if="kind === 'ssh'" class="grid grid-cols-2 gap-3">
                <div class="grid gap-1.5">
                  <Label for="add-repo-ssh-host" class="text-base">Host</Label>
                  <Input id="add-repo-ssh-host" v-model="ssh.host" class="text-base" placeholder="git.example.com" autocomplete="off" />
                </div>
                <div class="grid gap-1.5">
                  <Label for="add-repo-ssh-user" class="text-base">User</Label>
                  <Input id="add-repo-ssh-user" v-model="ssh.user" class="text-base" placeholder="deploy" autocomplete="off" />
                </div>
                <div class="grid gap-1.5">
                  <Label for="add-repo-ssh-port" class="text-base">Port</Label>
                  <Input id="add-repo-ssh-port" v-model="ssh.port" class="text-base" placeholder="22" inputmode="numeric" autocomplete="off" />
                </div>
                <div class="grid gap-1.5">
                  <Label for="add-repo-ssh-identity" class="text-base">Identity file</Label>
                  <Input id="add-repo-ssh-identity" v-model="ssh.identityFile" class="font-mono text-base" placeholder="~/.ssh/id_ed25519" autocomplete="off" />
                </div>
                <div class="col-span-2 grid gap-1.5">
                  <Label for="add-repo-ssh-git" class="text-base">Remote git path</Label>
                  <Input id="add-repo-ssh-git" v-model="ssh.remoteGit" class="font-mono text-base" placeholder="/usr/bin/git" autocomplete="off" />
                </div>
              </div>
            </div>
          </CollapsibleContent>
        </Collapsible>

        <p v-if="submitError" class="rounded-md border border-deleted/40 bg-deleted/10 px-2.5 py-1.5 text-base text-deleted-fg">
          {{ submitError }}
        </p>
      </form>

      <DialogFooter>
        <Button type="button" variant="outline" size="sm" @click="open = false">Cancel</Button>
        <Button type="button" size="sm" :disabled="!canSubmit" @click="submit">
          <LoaderCircleIcon v-if="submitting" class="animate-spin" />
          Add
        </Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
</template>
