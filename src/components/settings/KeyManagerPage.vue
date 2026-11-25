<script setup lang="ts">
import { computed, onMounted, ref, watch } from "vue";
import { join } from "@tauri-apps/api/path";
import { open as openDialog, save as saveDialog } from "@tauri-apps/plugin-dialog";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { FolderPlusIcon, KeyRoundIcon, LoaderCircleIcon, RefreshCwIcon, UploadIcon, XIcon } from "@lucide/vue";
import { commands, type ExportFormat, type GenerateAlgorithm, type KeychainStatus, type KeyFolder, type KeyInfo, type KeyPlace, type RemoteKeys, type RepoEntry } from "@/lib/bindings";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Tabs, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { Tip } from "@/components/ui/tooltip";
import { pathBasename } from "@/lib/format";
import { errorMessage, unwrap } from "@/lib/ipc";
import { connectionWithKey, extraKeyDirs, isSshEntry, keychainStatus, placeId, placeLabel, scanKeys, withPassphrase } from "@/lib/keys";
import { openRepository } from "@/lib/repoActions";
import { cn } from "@/lib/utils";
import { useDialogsStore } from "@/stores/dialogs";
import { repoDisplayName, useLibraryStore } from "@/stores/library";
import { useSessionsStore } from "@/stores/sessions";
import { useSettingsStore } from "@/stores/settings";
import KeyChangePassphraseDialog from "./KeyChangePassphraseDialog.vue";
import KeyCopyDialog, { type KeyCopyRequest, type KeyCopyTarget } from "./KeyCopyDialog.vue";
import KeyExportDialog from "./KeyExportDialog.vue";
import KeyImportDropzone from "./KeyImportDropzone.vue";
import KeyPassphraseFields from "./KeyPassphraseFields.vue";
import KeyRow, { type KeyAction } from "./KeyRow.vue";

const dialogs = useDialogsStore();
const library = useLibraryStore();
const sessions = useSessionsStore();
const settingsStore = useSettingsStore();

const keys = ref<KeyInfo[] | null>(null);
const loading = ref(false);
/** Every folder read, default and added, with the tab it belongs in. */
const folders = ref<KeyFolder[]>([]);
const defaultDirs = computed(() => folders.value.filter((f) => !f.extra).map((f) => f.path));
/** `~/.ssh` on the server of each open repository reached over SSH. */
const remote = ref<RemoteKeys[]>([]);
const extraDirs = computed(() => extraKeyDirs());
const agentAvailable = ref(false);
const keychain = ref<KeychainStatus | null>(null);
/** The key an action is running on. */
const busyPath = ref<string | null>(null);
/** The outcome of the last action. The page sits in a dialog, so the window's notices would be hidden behind it. */
const message = ref<{ kind: "success" | "error"; text: string } | null>(null);

/** The form shown above the list, if any: the two that make a new key. What is done to one key opens a dialog. */
type Panel = "generate" | "import";
const panel = ref<Panel | null>(null);
/** The key the Copy dialog is open for, with the places it can go. */
const copyRequest = ref<KeyCopyRequest | null>(null);
/** The key the Export dialog is open for. */
const exportKey = ref<KeyInfo | null>(null);
/** The key the Change Passphrase dialog is open for. */
const changePassphraseKey = ref<KeyInfo | null>(null);

const sshRepos = computed<RepoEntry[]>(() => library.repos.filter(isSshEntry));

/** One tab: Windows, a WSL distro, this computer, or a server. */
interface PlaceTab {
  id: string;
  label: string;
  place: KeyPlace;
  folders: KeyFolder[];
  keys: KeyInfo[];
  /** Where a copy into this place is written: its first folder, or null for `~/.ssh` of a WSL
   *  distro reached from Windows or of a server, which the backend finds itself. */
  writeFolder: string | null;
  /** Why a server's keys could not be read. */
  error: string | null;
}

const PLACE_ORDER: Record<KeyPlace["type"], number> = { windows: 0, wsl: 1, local: 2, remote: 3 };

const tabs = computed<PlaceTab[]>(() => {
  const byId = new Map<string, PlaceTab>();
  const tabFor = (place: KeyPlace): PlaceTab => {
    const id = placeId(place);
    let tab = byId.get(id);
    if (!tab) {
      tab = { id, label: placeLabel(place), place, folders: [], keys: [], writeFolder: null, error: null };
      byId.set(id, tab);
    }
    return tab;
  };
  for (const f of folders.value) tabFor(f.place).folders.push(f);
  for (const k of keys.value ?? []) tabFor(k.place).keys.push(k);
  for (const r of remote.value) {
    const tab = tabFor({ type: "remote", repoKey: r.repoKey, server: r.server });
    tab.keys.push(...r.keys);
    tab.error = r.error;
    if (r.folder) tab.folders.push({ path: r.folder, place: tab.place, extra: false });
  }
  for (const tab of byId.values()) {
    const first = tab.folders.find((f) => !f.extra)?.path ?? tab.folders[0]?.path ?? null;
    // A WSL distro seen from Windows is written to through the distro, so its own permissions apply.
    const throughShell = tab.place.type === "remote" || (tab.place.type === "wsl" && (first ?? "\\\\").startsWith("\\\\"));
    tab.writeFolder = throughShell ? null : first;
  }
  return [...byId.values()].sort((a, b) => PLACE_ORDER[a.place.type] - PLACE_ORDER[b.place.type] || a.label.localeCompare(b.label));
});

const activeTab = ref("");
watch(tabs, (list) => {
  if (!list.some((t) => t.id === activeTab.value)) activeTab.value = list[0]?.id ?? "";
});
const shown = computed(() => tabs.value.find((t) => t.id === activeTab.value) ?? null);

/** Where a key can be copied to: every place but its own that can be written to. */
function copyTargets(key: KeyInfo): PlaceTab[] {
  const own = placeId(key.place);
  return tabs.value.filter((t) => t.id !== own && t.error === null && (t.writeFolder !== null || t.place.type === "remote" || t.place.type === "wsl"));
}

/** The folder new keys go in: the first default one (`~/.ssh`). */
const firstFolder = computed(() => defaultDirs.value[0] ?? extraDirs.value[0] ?? null);

function succeed(text: string): void {
  message.value = { kind: "success", text };
}

function fail(e: unknown): void {
  message.value = { kind: "error", text: errorMessage(e) };
}

async function refresh(): Promise<void> {
  loading.value = true;
  try {
    const [found, dirs, servers] = await Promise.all([
      scanKeys(),
      unwrap(commands.keysFolders(extraKeyDirs())),
      unwrap(commands.keysScanRemote()).catch((): RemoteKeys[] => []),
    ]);
    keys.value = found;
    folders.value = dirs;
    remote.value = servers;
  } catch (e) {
    keys.value ??= [];
    fail(e);
  } finally {
    loading.value = false;
  }
  try {
    await unwrap(commands.keysAgentList());
    agentAvailable.value = true;
  } catch {
    agentAvailable.value = false;
  }
}

onMounted(() => {
  void refresh();
  keychainStatus()
    .then((status) => (keychain.value = status))
    .catch(() => (keychain.value = null));
});

async function addFolder(): Promise<void> {
  try {
    const picked = await openDialog({ directory: true, multiple: false, title: "Choose a folder with SSH keys" });
    if (typeof picked !== "string" || picked.length === 0) return;
    if (defaultDirs.value.includes(picked) || extraDirs.value.includes(picked)) return;
    // Arrays replace wholesale in a merge patch, so send the whole list.
    await settingsStore.update({ keys: { extraDirs: [...extraDirs.value, picked] } });
    await refresh();
  } catch (e) {
    fail(e);
  }
}

async function removeFolder(dir: string): Promise<void> {
  try {
    await settingsStore.update({ keys: { extraDirs: extraDirs.value.filter((d) => d !== dir) } });
    await refresh();
  } catch (e) {
    fail(e);
  }
}

/** Runs an action on one key with its row marked busy; failures land in the page's message. */
async function onKey(info: KeyInfo, run: () => Promise<void>): Promise<void> {
  busyPath.value = info.path;
  message.value = null;
  try {
    await run();
  } catch (e) {
    fail(e);
  } finally {
    busyPath.value = null;
  }
}

function onAction(info: KeyInfo, action: KeyAction): void {
  switch (action) {
    case "export":
      message.value = null;
      exportKey.value = info;
      return;
    case "changePassphrase":
      message.value = null;
      changePassphraseKey.value = info;
      return;
    case "convert":
      void onKey(info, async () => {
        // The new file keeps the original's passphrase, if it has one.
        const written = await withPassphrase(info, "to convert it", (p) => unwrap(commands.keysConvert(info.path, "openSsh", null, p, null, false)));
        if (written === undefined) return;
        await refresh();
        succeed(`Converted ${info.fileName} to OpenSSH format as ${pathBasename(written)}. The original is unchanged.`);
      });
      return;
    case "copyPublic":
      void onKey(info, async () => {
        const line = await withPassphrase(info, "to read its public key", (p) => unwrap(commands.keysExportPublic(info.path, p)));
        if (line === undefined) return;
        await navigator.clipboard.writeText(line);
        succeed(`Copied the public key of ${info.fileName}. Paste it into the server's authorized_keys file.`);
      });
      return;
    case "addToAgent":
    case "addToAgentForHours":
      void onKey(info, async () => {
        const lifetime = action === "addToAgentForHours" ? 8 * 60 * 60 : null;
        const added = await withPassphrase(info, "to add it to the SSH agent", (p) => unwrap(commands.keysAgentAdd(info.path, p, lifetime)));
        if (added === undefined) return;
        await refresh();
        succeed(lifetime ? `Added ${info.fileName} to the SSH agent for 8 hours.` : `Added ${info.fileName} to the SSH agent.`);
      });
      return;
    case "forgetPassphrase":
      void onKey(info, async () => {
        await unwrap(commands.keysForgetPassphrase(info.path));
        await refresh();
        succeed(`Forgot the passphrase of ${info.fileName}.`);
      });
      return;
    case "copyTo":
      message.value = null;
      copyRequest.value = { key: info, targets: copyTargets(info) };
      return;
    case "reveal":
      void onKey(info, () => revealItemInDir(info.path));
      return;
    case "delete":
      void onKey(info, async () => {
        const confirmed = await dialogs.open("confirm", {
          title: `Delete ${info.fileName}?`,
          message:
            `${info.path}\n\nThe private key file is deleted` +
            (info.publicKeyFile ? `, and its public key file ${pathBasename(info.publicKeyFile)} with it` : "") +
            ". This cannot be undone. Servers that only accept this key will refuse you afterwards.",
          confirmLabel: "Delete",
          destructive: true,
        });
        if (!confirmed) return;
        await unwrap(commands.keysDelete(info.path, true));
        await refresh();
        succeed(`Deleted ${info.fileName}.`);
      });
      return;
  }
}

/** Sets a library entry to log in with this key, and reopens its tab so the change takes effect. */
function useFor(info: KeyInfo, entry: RepoEntry): void {
  if (!entry.key) return;
  const key = entry.key;
  void onKey(info, async () => {
    const updated = await library.updateRepo(key, entry.path ?? "", connectionWithKey(entry, info.path), entry.displayName ?? null);
    const name = repoDisplayName(updated);
    if (sessions.sessions.has(key) && updated.key) {
      await sessions.closeTab(key);
      void openRepository(updated.key);
      succeed(`${name} now logs in with ${info.fileName}. Its tab was reopened.`);
    } else {
      succeed(`${name} now logs in with ${info.fileName}.`);
    }
  });
}

// The Generate form, filled in when it opens. Import has a component of its own.

const form = ref({
  algorithm: "ed25519" as GenerateAlgorithm,
  comment: "",
  outPath: "",
  outPathEdited: false,
  passphrase: "",
  passphraseAgain: "",
});
const formError = ref<string | null>(null);
const formBusy = ref(false);

function openPanel(p: Panel): void {
  panel.value = p;
  message.value = null;
  formError.value = null;
  form.value = { algorithm: "ed25519", comment: form.value.comment, outPath: "", outPathEdited: false, passphrase: "", passphraseAgain: "" };
  if (p === "generate" && form.value.comment === "") {
    unwrap(commands.keysDefaultComment())
      .then((c) => (form.value.comment = c))
      .catch(() => undefined);
  }
}

/** `id_ed25519`, or `id_ed25519_2` and so on when a scanned key in that folder has the name. */
function freeKeyName(base: string): string {
  const folder = firstFolder.value;
  const taken = new Set((keys.value ?? []).filter((k) => folder && k.path.startsWith(folder)).map((k) => k.fileName));
  if (!taken.has(base)) return base;
  for (let n = 2; ; n++) if (!taken.has(`${base}_${n}`)) return `${base}_${n}`;
}

// Generate: the file name follows the algorithm until the user edits the path.
watch(
  () => [panel.value, form.value.algorithm, firstFolder.value] as const,
  async ([kind, algorithm, folder]) => {
    if (kind !== "generate" || form.value.outPathEdited) return;
    const name = freeKeyName(algorithm === "ed25519" ? "id_ed25519" : "id_rsa");
    form.value.outPath = folder ? await join(folder, name).catch(() => name) : name;
  },
);

/** An empty passphrase is allowed: the key is then written unencrypted. */
const formReady = computed(
  () => panel.value === "generate" && !formBusy.value && form.value.passphrase === form.value.passphraseAgain && form.value.outPath.trim().length > 0,
);

async function browseGeneratePath(): Promise<void> {
  try {
    const picked = await saveDialog({ title: "Save the new key as", defaultPath: form.value.outPath || undefined });
    if (typeof picked === "string" && picked.length > 0) {
      form.value.outPath = picked;
      form.value.outPathEdited = true;
    }
  } catch (e) {
    formError.value = errorMessage(e);
  }
}

async function submitForm(): Promise<void> {
  if (!formReady.value) return;
  formBusy.value = true;
  formError.value = null;
  try {
    const info = await unwrap(
      commands.keysGenerate(form.value.algorithm, form.value.comment.trim(), form.value.outPath.trim(), form.value.passphrase || null, false),
    );
    panel.value = null;
    await refresh();
    succeed(`Generated ${info.fileName}, with ${info.fileName}.pub beside it. Use Copy public key to put it on a server.`);
  } catch (e) {
    formError.value = errorMessage(e);
  } finally {
    formBusy.value = false;
  }
}

/** The Export dialog wrote the file: close it, read the keys again and say where the export went. */
async function onExported(key: KeyInfo, written: string, format: ExportFormat): Promise<void> {
  exportKey.value = null;
  await refresh();
  succeed(`Exported ${key.fileName} in ${format === "pkcs8" ? "PKCS#8" : "OpenSSH"} format as ${written}, with a .pub file beside it.`);
}

/** The Change Passphrase dialog wrote the key file again: close it and read the keys again. */
async function onPassphraseChanged(info: KeyInfo, removed: boolean): Promise<void> {
  changePassphraseKey.value = null;
  await refresh();
  succeed(removed ? `Removed the passphrase of ${info.fileName}. Anyone who can read the file can now use the key.` : `Changed the passphrase of ${info.fileName}.`);
}

/** The Copy dialog wrote the copy: close it, read the keys again and say where the copy went. */
async function onCopied(key: KeyInfo, target: KeyCopyTarget, written: string): Promise<void> {
  copyRequest.value = null;
  await refresh();
  succeed(`Copied ${key.fileName} to ${target.label} as ${written}${key.publicKeyFile ? ", with its .pub file" : ""}.`);
}

async function onImported(info: KeyInfo): Promise<void> {
  panel.value = null;
  await refresh();
  succeed(`Imported the key as ${info.path}.`);
}

const selectClass =
  "well h-8 w-full rounded-lg border px-2.5 text-base outline-none focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/50";
</script>

<template>
  <div class="grid gap-4">
    <div class="flex flex-wrap items-center gap-2">
      <Button type="button" variant="outline" size="sm" @click="openPanel('generate')">
        <KeyRoundIcon />
        Generate Key…
      </Button>
      <Button type="button" variant="outline" size="sm" @click="openPanel('import')">
        <UploadIcon />
        Import Key…
      </Button>
      <Button type="button" variant="outline" size="sm" @click="addFolder">
        <FolderPlusIcon />
        Add Folder…
      </Button>
      <Button type="button" variant="ghost" size="sm" class="ml-auto" :disabled="loading" @click="refresh">
        <RefreshCwIcon :class="cn(loading && 'animate-spin')" />
        Refresh
      </Button>
    </div>

    <Tabs v-if="tabs.length > 1" v-model="activeTab">
      <TabsList class="flex-wrap justify-start">
        <TabsTrigger v-for="t in tabs" :key="t.id" :value="t.id" class="text-base" :data-key-place="t.id">
          {{ t.label }}<span class="ml-1 text-muted-foreground">({{ t.keys.length }})</span>
        </TabsTrigger>
      </TabsList>
    </Tabs>

    <div class="grid gap-1 text-xs text-muted-foreground">
      <p>{{ shown?.place.type === "remote" ? `Keys on ${shown.label}, read through the connection of an open repository:` : "Looking for keys in:" }}</p>
      <ul class="grid gap-0.5">
        <li v-for="f in shown?.folders ?? []" :key="f.path" class="flex min-w-0 items-center gap-1">
          <Tip :content="f.path">
            <span class="truncate font-mono">{{ f.path }}</span>
          </Tip>
          <Tip v-if="f.extra" :content="`Stop looking in ${f.path}`">
            <button type="button" class="shrink-0 rounded p-0.5 hover:bg-row-hover hover:text-foreground" :aria-label="`Stop looking in ${f.path}`" @click="removeFolder(f.path)">
              <XIcon class="size-3" />
            </button>
          </Tip>
        </li>
        <li v-if="shown?.error" class="text-destructive">{{ shown.error }}</li>
        <li v-else-if="(shown?.folders.length ?? 0) === 0">No .ssh folder was found. Generate or import a key to create one.</li>
      </ul>
    </div>

    <p
      v-if="message"
      :class="
        cn(
          'rounded-md border px-2.5 py-1.5 text-base break-words',
          message.kind === 'error' ? 'border-destructive/40 bg-destructive/10 text-destructive' : 'border-border bg-muted/30',
        )
      "
      role="status"
    >
      {{ message.text }}
    </p>

    <KeyImportDropzone v-if="panel === 'import'" :folder="firstFolder" @imported="onImported" @cancel="panel = null" />

    <form v-else-if="panel === 'generate'" class="grid gap-3 rounded-lg border border-border p-3" @submit.prevent="submitForm">
      <h4 class="text-base font-medium">Generate a key</h4>
      <div class="grid grid-cols-2 gap-3">
        <div class="grid gap-1.5">
          <Label for="key-generate-algorithm" class="text-base">Type</Label>
          <select id="key-generate-algorithm" v-model="form.algorithm" :class="selectClass">
            <option value="ed25519">Ed25519 (recommended)</option>
            <option value="rsa4096">RSA 4096, for servers that do not take Ed25519</option>
          </select>
        </div>
        <div class="grid gap-1.5">
          <Label for="key-generate-comment" class="text-base">Comment</Label>
          <Input id="key-generate-comment" v-model="form.comment" class="text-base" autocomplete="off" spellcheck="false" />
        </div>
      </div>
      <div class="grid gap-1.5">
        <Label for="key-generate-path" class="text-base">Save as</Label>
        <div class="flex gap-2">
          <Input id="key-generate-path" v-model="form.outPath" class="font-mono text-base" autocomplete="off" spellcheck="false" @input="form.outPathEdited = true" />
          <Button type="button" variant="outline" size="sm" class="h-8 shrink-0" @click="browseGeneratePath">Browse…</Button>
        </div>
      </div>
      <KeyPassphraseFields v-model:passphrase="form.passphrase" v-model:again="form.passphraseAgain" id-prefix="key-generate" optional />

      <p v-if="formError" class="rounded-md border border-destructive/40 bg-destructive/10 px-2.5 py-1.5 text-base break-words text-destructive">
        {{ formError }}
      </p>

      <div class="flex justify-end gap-2">
        <Button type="button" variant="outline" @click="panel = null">Cancel</Button>
        <Button type="submit" :disabled="!formReady">
          <LoaderCircleIcon v-if="formBusy" class="animate-spin" />
          Generate
        </Button>
      </div>
    </form>

    <div v-if="keys === null" class="flex items-center gap-2 text-base text-muted-foreground">
      <LoaderCircleIcon class="size-3.5 animate-spin" />
      Looking for keys…
    </div>
    <p v-else-if="(shown?.keys.length ?? 0) === 0" class="text-base text-muted-foreground">
      {{ shown?.place.type === "remote" ? "No private keys were found in ~/.ssh on this server." : "No private keys were found. Generate one, or import one you have." }}
    </p>
    <div v-else class="grid gap-2">
      <KeyRow
        v-for="k in shown?.keys ?? []"
        :key="k.path"
        :info="k"
        :ssh-repos="sshRepos"
        :agent-available="agentAvailable"
        :busy="busyPath === k.path"
        @action="(a) => onAction(k, a)"
        @use-for="(entry) => useFor(k, entry)"
      />
    </div>

    <p class="text-xs text-muted-foreground">
      <template v-if="keychain?.available">Passphrases can be stored in {{ keychain.name }}, so they are remembered after a restart.</template>
      <template v-else>
        No system keychain is available here, so passphrases are remembered only until StashPanda closes.
      </template>
      <template v-if="!agentAvailable"> No SSH agent answered, so keys cannot be added to one.</template>
    </p>

    <KeyCopyDialog :request="copyRequest" @close="copyRequest = null" @copied="onCopied" />
    <KeyExportDialog :key-info="exportKey" @close="exportKey = null" @exported="onExported" />
    <KeyChangePassphraseDialog :key-info="changePassphraseKey" @close="changePassphraseKey = null" @changed="onPassphraseChanged" />
  </div>
</template>
