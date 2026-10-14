<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from "vue";
import { useDebounceFn } from "@vueuse/core";
import { homeDir, join } from "@tauri-apps/api/path";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import type { UnlistenFn } from "@tauri-apps/api/event";
import { ChevronDownIcon, ChevronRightIcon, EllipsisIcon, FolderOpenIcon, LoaderCircleIcon, TriangleAlertIcon } from "@lucide/vue";
import {
  commands,
  events,
  type CloneOutcome,
  type CloneTarget,
  type Connection,
  type RepoEntry,
  type RepoResolution,
} from "@/lib/bindings";
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
import { Tabs, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { pathBasename, transportLabel } from "@/lib/format";
import { errorMessage, IpcError, unwrap } from "@/lib/ipc";
import { removeFromLibrary } from "@/lib/repoActions";
import { useDialogsStore, type AddRepoTab } from "@/stores/dialogs";
import { useHostStore } from "@/stores/host";
import { repoDisplayName, useLibraryStore } from "@/stores/library";
import { useSessionsStore } from "@/stores/sessions";

const open = defineModel<boolean>("open", { default: false });

const dialogs = useDialogsStore();
const host = useHostStore();
const library = useLibraryStore();
const sessions = useSessionsStore();

type ConnectionKind = Connection["type"];

/** Add Existing adds a folder that is a repository already; Clone From clones a remote into one. */
const tab = ref<AddRepoTab>("existing");
const url = ref("");
const path = ref("");
const displayName = ref("");
const kind = ref<ConnectionKind>("auto");
const wslDistro = ref("");
const ssh = ref({ host: "", user: "", port: "", identityFile: "", remoteGit: "" });
const advancedOpen = ref(false);
/**
 * The library entry being edited, or null when adding. Taken from the store when the dialog opens
 * and kept until the next open, so the title does not flip back to "Add" while the dialog fades out.
 */
const editingKey = ref<string | null>(null);

const distros = ref<string[]>([]);
const isWindows = computed(() => host.info?.type === "windows");

/** True on the Clone From tab. Editing an entry has no tabs, so it is never a clone. */
const isClone = computed(() => tab.value === "clone" && editingKey.value === null);

const resolution = ref<RepoResolution | null>(null);
/** What the Clone From tab found at the folder; null on Add Existing. */
const cloneTarget = ref<CloneTarget | null>(null);
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

/** Resolves the path. On Clone From it also looks at what is in the folder now. */
async function resolveNow(): Promise<void> {
  const p = path.value.trim();
  if (p.length === 0 || !connectionReady.value) {
    resolution.value = null;
    cloneTarget.value = null;
    resolveError.value = null;
    resolving.value = false;
    return;
  }
  const serial = ++resolveSerial;
  resolving.value = true;
  try {
    if (isClone.value) {
      const target = await unwrap(commands.cloneInspect(p, connection.value));
      if (serial !== resolveSerial) return;
      cloneTarget.value = target;
      resolution.value = target.resolution;
    } else {
      const result = await unwrap(commands.repoResolve(p, connection.value));
      if (serial !== resolveSerial) return;
      cloneTarget.value = null;
      resolution.value = result;
    }
    resolveError.value = null;
  } catch (e) {
    if (serial !== resolveSerial) return;
    resolution.value = null;
    cloneTarget.value = null;
    resolveError.value = errorMessage(e);
  } finally {
    if (serial === resolveSerial) resolving.value = false;
  }
}

const resolveDebounced = useDebounceFn(resolveNow, 300);

watch([path, connection, isClone], () => {
  submitError.value = null;
  resolving.value = path.value.trim().length > 0 && connectionReady.value;
  void resolveDebounced();
});

watch(open, async (isOpen) => {
  if (!isOpen) {
    dialogs.editRepoKey = null;
    dialogs.addRepoTab = "existing";
    return;
  }
  reset();
  const entry = dialogs.editRepoKey ? library.repoByKey.get(dialogs.editRepoKey) : undefined;
  editingKey.value = entry?.key ?? null;
  tab.value = entry ? "existing" : dialogs.addRepoTab;
  if (entry) fill(entry);
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
  url.value = "";
  path.value = "";
  cloneTarget.value = null;
  progress.value = null;
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

/** Puts an existing entry's values into the fields for editing. */
function fill(entry: RepoEntry): void {
  path.value = entry.path ?? "";
  displayName.value = entry.displayName ?? "";
  const c: Connection = entry.connection ?? { type: "auto" };
  kind.value = c.type;
  if (c.type === "wsl") wslDistro.value = c.distro;
  if (c.type === "ssh") {
    ssh.value = {
      host: c.host,
      user: c.user ?? "",
      port: c.port?.toString() ?? "",
      identityFile: c.identityFile ?? "",
      remoteGit: c.remoteGit ?? "",
    };
  }
  advancedOpen.value = c.type !== "auto";
}

async function browse(): Promise<void> {
  try {
    const title = isClone.value ? "Choose the folder to clone into" : "Choose a repository folder";
    const picked = await openDialog({ directory: true, multiple: false, title });
    if (typeof picked === "string" && picked.length > 0) path.value = picked;
  } catch (e) {
    submitError.value = errorMessage(e);
  }
}

async function browseIdentityFile(): Promise<void> {
  try {
    // Start in ~/.ssh, where keys usually live; the picker falls back to its own default if it is missing.
    const defaultPath = await join(await homeDir(), ".ssh").catch(() => undefined);
    const picked = await openDialog({ directory: false, multiple: false, title: "Choose an SSH key", defaultPath });
    if (typeof picked === "string" && picked.length > 0) ssh.value.identityFile = picked;
  } catch (e) {
    submitError.value = errorMessage(e);
  }
}

/**
 * The name a clone's folder usually gets: the last part of the URL without `.git`, as
 * `name_from_url` in `repo/clone.rs` works it out. `git@github.com:django/django.git` gives `django`.
 */
const urlName = computed<string | null>(() => {
  let u = url.value.trim().replace(/[\\/]+$/, "");
  if (u.endsWith(".git")) u = u.slice(0, -4).replace(/[\\/]+$/, "");
  const last = u.split(/[\\/:]/).pop() ?? "";
  return last.length > 0 ? last : null;
});

/**
 * A folder named after the repository inside the chosen one, offered when the chosen folder
 * already has files or is a repository and has a different name: picking the parent folder in
 * the file browser is the usual slip.
 */
const suggestedPath = computed<string | null>(() => {
  const folder = cloneTarget.value?.folder.type;
  const name = urlName.value;
  const p = path.value.trim().replace(/[\\/]+$/, "");
  if ((folder !== "hasFiles" && folder !== "repository") || !name || p.length === 0) return null;
  if (pathBasename(p).toLowerCase() === name.toLowerCase()) return null;
  const sep = p.includes("\\") && !p.includes("/") ? "\\" : "/";
  return `${p}${sep}${name}`;
});

/** The clone's latest progress while it runs. */
const progress = ref<{ opId: string; line: string; cancellable: boolean; stopping: boolean } | null>(null);
/** True while a clone runs: the fields are locked and the dialog cannot be closed, only stopped. */
const cloneRunning = computed(() => progress.value !== null);

let unlistenProgress: UnlistenFn | null = null;
void events.cloneProgress
  .listen((event) => {
    const p = progress.value;
    if (!p || p.opId !== event.payload.opId) return;
    progress.value = { ...p, line: event.payload.line, cancellable: event.payload.cancellable };
  })
  .then((unlisten) => {
    unlistenProgress = unlisten;
  });
onBeforeUnmount(() => unlistenProgress?.());

/** Keeps the dialog open while a clone runs: Escape and the close button do nothing then. */
const dialogOpen = computed({
  get: () => open.value,
  set: (value: boolean) => {
    if (!value && cloneRunning.value) return;
    open.value = value;
  },
});

const canSubmit = computed(() => {
  if (path.value.trim().length === 0 || !connectionReady.value || submitting.value) return false;
  if (!isClone.value) return true;
  const folder = cloneTarget.value?.folder.type;
  return url.value.trim().length > 0 && !resolving.value && folder !== undefined && folder !== "repository";
});

async function submit(): Promise<void> {
  if (!canSubmit.value) return;
  submitting.value = true;
  submitError.value = null;
  try {
    if (isClone.value) {
      await submitClone();
      return;
    }
    if (editingKey.value) {
      await saveEdit(editingKey.value);
      return;
    }
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

async function submitClone(): Promise<void> {
  const opId = crypto.randomUUID();
  progress.value = { opId, line: "Starting the clone…", cancellable: true, stopping: false };
  try {
    const result = await library.cloneRepo(
      opId,
      url.value.trim(),
      path.value.trim(),
      connection.value,
      displayName.value.trim() || null,
    );
    if (!result.entry.key) throw new Error("The backend returned a library entry without a key.");
    progress.value = null;
    const cloned = `Cloned ${url.value.trim()} into ${result.entry.path ?? path.value.trim()}.`;
    open.value = false;
    await sessions.openTab(result.entry.key);
    const notice = cloneNotice(result.outcome) ?? { kind: "success", message: cloned };
    if (notice?.kind === "success") sessions.notifySuccess(notice.message);
    else if (notice) sessions.notify(notice.message);
  } catch (e) {
    submitError.value =
      e instanceof IpcError && e.error.kind === "cancelled"
        ? "The clone was stopped. The folder is as it was before."
        : errorMessage(e);
    // Whatever the clone did to the folder has been undone, so look at it again.
    void resolveNow();
  } finally {
    progress.value = null;
  }
}

/** What to say after a clone, when there is more to say than the new tab shows. */
function cloneNotice(outcome: CloneOutcome): { message: string; kind: "error" | "success" } | null {
  const parts: string[] = [];
  if (outcome.keptExisting) {
    parts.push(
      "The folder's own files were kept. Where one differs from the repository's file it shows as a change, and files the repository does not have show as untracked.",
    );
  }
  if (outcome.blocked.length > 0) {
    const shown = outcome.blocked.slice(0, 3).join(", ");
    const more = outcome.blocked.length > 3 ? ` and ${outcome.blocked.length - 3} more` : "";
    parts.push(
      `${outcome.blocked.length} of the repository's files were not written, because a file in the folder is where their folder would go: ${shown}${more}. They show as deleted.`,
    );
  }
  parts.push(...outcome.warnings);
  if (parts.length === 0) return null;
  const problem = outcome.blocked.length > 0 || outcome.warnings.length > 0;
  return { message: parts.join(" "), kind: problem ? "error" : "success" };
}

async function stopClone(): Promise<void> {
  const p = progress.value;
  if (!p || !p.cancellable || p.stopping) return;
  progress.value = { ...p, stopping: true, line: "Stopping…" };
  try {
    await unwrap(commands.cloneCancel(p.opId));
  } catch (e) {
    submitError.value = errorMessage(e);
  }
}

async function saveEdit(key: string): Promise<void> {
  const before = library.repoByKey.get(key);
  const entry = await library.updateRepo(key, path.value.trim(), connection.value, displayName.value.trim() || null);
  if (!entry.key) throw new Error("The backend returned a library entry without a key.");
  // The old key is gone once the path or connection moved the entry, so a retry must use the new one.
  editingKey.value = entry.key;
  const session = sessions.sessions.get(key);
  if (session) {
    const moved =
      entry.key !== key ||
      entry.path !== before?.path ||
      JSON.stringify(entry.connection) !== JSON.stringify(before?.connection);
    if (moved) {
      // The open tab talks to the old location, so it is reopened against the new one.
      await sessions.closeTab(key);
      await sessions.openTab(entry.key);
    } else {
      session.info = { ...session.info, displayName: repoDisplayName(entry) };
    }
  }
  open.value = false;
}

/** The same removal as the Repositories context menu; the dialog closes once it is done. */
async function removeEntry(): Promise<void> {
  const key = editingKey.value;
  if (!key) return;
  submitting.value = true;
  try {
    if (await removeFromLibrary(key)) open.value = false;
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
  "well h-8 w-full rounded-lg border px-2.5 text-base outline-none focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/50";
</script>

<template>
  <Dialog v-model:open="dialogOpen">
    <DialogContent class="sm:max-w-2xl" size-key="addRepo" :close-on-outside-click="false" :show-close-button="!cloneRunning">
      <DialogHeader>
        <DialogTitle>{{ editingKey ? "Edit repository" : "Add repository" }}</DialogTitle>
        <DialogDescription v-if="isClone">
          Clone a remote repository into a Windows, UNC, Linux or <span class="font-mono">user@host:/path</span> folder.
          The folder can already have files in it: they are kept.
        </DialogDescription>
        <DialogDescription v-else>
          Enter a Windows, UNC, Linux or <span class="font-mono">user@host:/path</span> location.
        </DialogDescription>
      </DialogHeader>

      <Tabs v-if="!editingKey" v-model="tab">
        <TabsList class="w-full">
          <TabsTrigger value="existing" class="text-base" :disabled="cloneRunning">Add Existing</TabsTrigger>
          <TabsTrigger value="clone" class="text-base" :disabled="cloneRunning">Clone From</TabsTrigger>
        </TabsList>
      </Tabs>

      <form class="grid gap-3" @submit.prevent="submit">
        <fieldset class="contents" :disabled="cloneRunning">
          <div v-if="isClone" class="grid gap-1.5">
            <Label for="add-repo-url" class="text-base">Repository URL</Label>
            <Input
              id="add-repo-url"
              v-model="url"
              class="font-mono text-base"
              placeholder="git@github.com:django/django.git"
              autocomplete="off"
              spellcheck="false"
            />
          </div>

          <div class="grid gap-1.5">
            <Label for="add-repo-path" class="text-base">{{ isClone ? "Folder to clone into" : "Repository path" }}</Label>
            <div class="flex gap-2">
              <Input
                id="add-repo-path"
                v-model="path"
                class="font-mono text-base"
                :placeholder="isClone ? 'Local, WSL or SSH folder; it is created if it does not exist' : 'Local, WSL or SSH path'"
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
            <p v-else-if="resolveError" class="text-destructive">{{ resolveError }}</p>
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
              <div v-if="cloneTarget" class="mt-1 grid gap-1">
                <p v-if="cloneTarget.folder.type === 'missing'" class="text-muted-foreground">
                  This folder does not exist yet. The clone creates it.
                </p>
                <p v-else-if="cloneTarget.folder.type === 'empty'" class="text-muted-foreground">This folder is empty.</p>
                <p v-else-if="cloneTarget.folder.type === 'hasFiles'" class="flex items-start gap-1.5 text-modified-fg">
                  <TriangleAlertIcon class="mt-px size-3.5 shrink-0" />
                  <span>
                    This folder has {{ cloneTarget.folder.count }} {{ cloneTarget.folder.count === 1 ? "item" : "items" }} in it
                    already. They are kept: the repository's files are added around them, and where a file of the folder's
                    differs from the repository's it shows as a change.
                  </span>
                </p>
                <p v-else class="flex items-start gap-1.5 text-destructive">
                  <TriangleAlertIcon class="mt-px size-3.5 shrink-0" />
                  <span>
                    This folder is a Git repository already, so it cannot be cloned into.
                    <button type="button" class="underline hover:text-foreground" @click="tab = 'existing'">
                      Add it with Add Existing
                    </button>
                    instead.
                  </span>
                </p>
                <p v-if="suggestedPath">
                  <button
                    type="button"
                    class="text-left font-mono text-xs underline text-muted-foreground hover:text-foreground"
                    @click="path = suggestedPath"
                  >
                    Clone into {{ suggestedPath }} instead
                  </button>
                </p>
              </div>
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
                    <Label for="add-repo-ssh-identity" class="text-base">SSH key</Label>
                    <div class="flex gap-2">
                      <Input id="add-repo-ssh-identity" v-model="ssh.identityFile" class="font-mono text-base" placeholder="~/.ssh/id_ed25519" autocomplete="off" />
                      <Button
                        type="button"
                        variant="outline"
                        size="icon"
                        class="shrink-0"
                        title="Choose an SSH key file"
                        aria-label="Choose an SSH key file"
                        @click="browseIdentityFile"
                      >
                        <EllipsisIcon />
                      </Button>
                    </div>
                  </div>
                  <div class="col-span-2 grid gap-1.5">
                    <Label for="add-repo-ssh-git" class="text-base">Remote git path</Label>
                    <Input id="add-repo-ssh-git" v-model="ssh.remoteGit" class="font-mono text-base" placeholder="/usr/bin/git" autocomplete="off" />
                    <p class="text-xs text-muted-foreground">
                      Where the <span class="font-mono">git</span> program is on the SSH host. Leave this empty to use the
                      <span class="font-mono">git</span> found on the host's <span class="font-mono">PATH</span>; fill it in only
                      if that is missing or is the wrong version.
                    </p>
                  </div>
                </div>
              </div>
            </CollapsibleContent>
          </Collapsible>

        </fieldset>

        <div v-if="progress" class="flex items-center gap-2 rounded-lg border border-border bg-muted/30 px-3 py-2 text-base">
          <LoaderCircleIcon class="size-3.5 shrink-0 animate-spin" />
          <span class="truncate font-mono text-xs" :title="progress.line">{{ progress.line }}</span>
        </div>
        <p v-else-if="submitError" class="rounded-md border border-destructive/40 bg-destructive/10 px-2.5 py-1.5 text-base text-destructive">
          {{ submitError }}
        </p>
      </form>

      <DialogFooter>
        <Button
          v-if="editingKey"
          type="button"
          variant="destructive"
          class="sm:mr-auto"
          title="Remove this repository from the library. The files on disk are not touched."
          :disabled="submitting"
          @click="removeEntry"
        >
          Remove
        </Button>
        <Button
          v-if="cloneRunning"
          type="button"
          variant="outline"
          :disabled="!progress?.cancellable || progress?.stopping"
          :title="progress?.cancellable ? 'Stop the clone. The folder is left as it was before.' : 'The files are being written out; this step runs to the end.'"
          @click="stopClone"
        >
          Stop
        </Button>
        <Button v-else type="button" variant="outline" @click="open = false">Cancel</Button>
        <Button type="button" :disabled="!canSubmit" @click="submit">
          <LoaderCircleIcon v-if="submitting" class="animate-spin" />
          {{ editingKey ? "Save" : isClone ? "Clone" : "Add" }}
        </Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
</template>
