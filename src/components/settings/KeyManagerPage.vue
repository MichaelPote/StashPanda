<script setup lang="ts">
import { computed, onMounted, ref, watch } from "vue";
import { join } from "@tauri-apps/api/path";
import { open as openDialog, save as saveDialog } from "@tauri-apps/plugin-dialog";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { FolderPlusIcon, KeyRoundIcon, LoaderCircleIcon, RefreshCwIcon, UploadIcon, XIcon } from "@lucide/vue";
import { commands, type ExportFormat, type GenerateAlgorithm, type KeychainStatus, type KeyInfo, type RepoEntry } from "@/lib/bindings";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { pathBasename } from "@/lib/format";
import { errorMessage, unwrap } from "@/lib/ipc";
import { connectionWithKey, extraKeyDirs, isSshEntry, keychainStatus, scanKeys, withPassphrase } from "@/lib/keys";
import { openRepository } from "@/lib/repoActions";
import { cn } from "@/lib/utils";
import { useDialogsStore } from "@/stores/dialogs";
import { repoDisplayName, useLibraryStore } from "@/stores/library";
import { useSessionsStore } from "@/stores/sessions";
import { useSettingsStore } from "@/stores/settings";
import KeyImportDropzone from "./KeyImportDropzone.vue";
import KeyRow, { type KeyAction } from "./KeyRow.vue";

const dialogs = useDialogsStore();
const library = useLibraryStore();
const sessions = useSessionsStore();
const settingsStore = useSettingsStore();

const keys = ref<KeyInfo[] | null>(null);
const loading = ref(false);
const defaultDirs = ref<string[]>([]);
const extraDirs = computed(() => extraKeyDirs());
const agentAvailable = ref(false);
const keychain = ref<KeychainStatus | null>(null);
/** The key an action is running on. */
const busyPath = ref<string | null>(null);
/** The outcome of the last action. The page sits in a dialog, so the window's notices would be hidden behind it. */
const message = ref<{ kind: "success" | "error"; text: string } | null>(null);

/** The form shown above the list, if any. */
type Panel =
  | { kind: "generate" }
  | { kind: "import" }
  | { kind: "export"; key: KeyInfo }
  | { kind: "changePassphrase"; key: KeyInfo };
const panel = ref<Panel | null>(null);

const sshRepos = computed<RepoEntry[]>(() => library.repos.filter(isSshEntry));

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
    const [found, dirs] = await Promise.all([scanKeys(), unwrap(commands.keysDefaultDirs())]);
    keys.value = found;
    defaultDirs.value = dirs;
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
      openPanel({ kind: "export", key: info });
      return;
    case "changePassphrase":
      openPanel({ kind: "changePassphrase", key: info });
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

// The forms. Each one is filled in when it opens.

const form = ref({
  algorithm: "ed25519" as GenerateAlgorithm,
  comment: "",
  outPath: "",
  outPathEdited: false,
  passphrase: "",
  passphraseAgain: "",
  /** Export and Change Passphrase: keep the passphrase, remove it, or set a new one. */
  protection: "same" as "same" | "none" | "new",
  format: "openSsh" as ExportFormat,
  current: "",
});
const formError = ref<string | null>(null);
const formBusy = ref(false);

function openPanel(p: Panel): void {
  panel.value = p;
  message.value = null;
  formError.value = null;
  form.value = {
    algorithm: "ed25519",
    comment: form.value.comment,
    outPath: "",
    outPathEdited: false,
    passphrase: "",
    passphraseAgain: "",
    protection: p.kind === "changePassphrase" ? "new" : "same",
    format: "openSsh",
    current: "",
  };
  if (p.kind === "generate" && form.value.comment === "") {
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
  () => [panel.value?.kind, form.value.algorithm, firstFolder.value] as const,
  async ([kind, algorithm, folder]) => {
    if (kind !== "generate" || form.value.outPathEdited) return;
    const name = freeKeyName(algorithm === "ed25519" ? "id_ed25519" : "id_rsa");
    form.value.outPath = folder ? await join(folder, name).catch(() => name) : name;
  },
);

const passphraseMismatch = computed(() => form.value.passphrase !== form.value.passphraseAgain);

/** The new passphrase as the commands take it: null keeps the current one, "" removes it. */
function chosenPassphrase(): string | null {
  switch (form.value.protection) {
    case "same":
      return null;
    case "none":
      return "";
    case "new":
      return form.value.passphrase;
  }
}

const formReady = computed(() => {
  const p = panel.value;
  if (!p || formBusy.value) return false;
  const needsNew = p.kind === "generate" || form.value.protection === "new";
  if (needsNew && passphraseMismatch.value) return false;
  if (p.kind !== "generate" && form.value.protection === "new" && form.value.passphrase.length === 0) return false;
  if (p.kind === "generate") return form.value.outPath.trim().length > 0;
  return true;
});

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
  const p = panel.value;
  if (!p || !formReady.value) return;
  formBusy.value = true;
  formError.value = null;
  try {
    if (p.kind === "generate") {
      const info = await unwrap(
        commands.keysGenerate(form.value.algorithm, form.value.comment.trim(), form.value.outPath.trim(), form.value.passphrase || null, false),
      );
      panel.value = null;
      await refresh();
      succeed(`Generated ${info.fileName}, with ${info.fileName}.pub beside it. Use Copy public key to put it on a server.`);
    } else if (p.kind === "export") {
      const formatName = form.value.format === "pkcs8" ? "PKCS#8" : "OpenSSH";
      const defaultPath = p.key.path.replace(/\.(ppk|pem|key)$/i, "") + (form.value.format === "pkcs8" ? ".pem" : "_openssh");
      const outPath = await saveDialog({ title: `Export ${p.key.fileName}`, defaultPath });
      if (typeof outPath !== "string" || outPath.length === 0) return;
      if (outPath === p.key.path) {
        formError.value = "Choose a different file: exporting over the original key would replace it.";
        return;
      }
      const newPassphrase = chosenPassphrase();
      // The save dialog has already asked about replacing an existing file.
      const written = await withPassphrase(p.key, "to export it", (pass) =>
        unwrap(commands.keysConvert(p.key.path, form.value.format, outPath, pass, newPassphrase, true)),
      );
      if (written === undefined) return;
      panel.value = null;
      await refresh();
      succeed(`Exported ${p.key.fileName} in ${formatName} format as ${written}, with a .pub file beside it.`);
    } else if (p.kind === "changePassphrase") {
      const current = form.value.current || null;
      const next = form.value.protection === "none" ? null : form.value.passphrase;
      const info = await withPassphrase(p.key, "to change it", (pass) => unwrap(commands.keysChangePassphrase(p.key.path, pass ?? current, next)));
      if (info === undefined) return;
      panel.value = null;
      await refresh();
      succeed(next ? `Changed the passphrase of ${info.fileName}.` : `Removed the passphrase of ${info.fileName}. Anyone who can read the file can now use the key.`);
    }
  } catch (e) {
    formError.value = errorMessage(e);
  } finally {
    formBusy.value = false;
  }
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
      <Button type="button" variant="outline" size="sm" @click="openPanel({ kind: 'generate' })">
        <KeyRoundIcon />
        Generate Key…
      </Button>
      <Button type="button" variant="outline" size="sm" @click="openPanel({ kind: 'import' })">
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

    <div class="grid gap-1 text-xs text-muted-foreground">
      <p>Looking for keys in:</p>
      <ul class="grid gap-0.5">
        <li v-for="dir in defaultDirs" :key="dir" class="truncate font-mono" :title="dir">{{ dir }}</li>
        <li v-for="dir in extraDirs" :key="dir" class="flex min-w-0 items-center gap-1">
          <span class="truncate font-mono" :title="dir">{{ dir }}</span>
          <button type="button" class="shrink-0 rounded p-0.5 hover:bg-row-hover hover:text-foreground" :aria-label="`Stop looking in ${dir}`" :title="`Stop looking in ${dir}`" @click="removeFolder(dir)">
            <XIcon class="size-3" />
          </button>
        </li>
        <li v-if="defaultDirs.length === 0 && extraDirs.length === 0">No .ssh folder was found. Generate or import a key to create one.</li>
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

    <KeyImportDropzone v-if="panel?.kind === 'import'" :folder="firstFolder" @imported="onImported" @cancel="panel = null" />

    <form v-else-if="panel" class="grid gap-3 rounded-lg border border-border p-3" @submit.prevent="submitForm">
      <template v-if="panel.kind === 'generate'">
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
      </template>

      <template v-else-if="panel.kind === 'export'">
        <h4 class="text-base font-medium">Export {{ panel.key.fileName }}</h4>
        <div class="grid gap-1.5">
          <Label for="key-export-format" class="text-base">Format</Label>
          <select id="key-export-format" v-model="form.format" :class="selectClass">
            <option value="openSsh">OpenSSH (for ssh, Git and most tools)</option>
            <option value="pkcs8">PKCS#8 PEM (for tools built on OpenSSL)</option>
          </select>
        </div>
      </template>

      <template v-else-if="panel.kind === 'changePassphrase'">
        <h4 class="text-base font-medium">Change the passphrase of {{ panel.key.fileName }}</h4>
        <div v-if="panel.key.encrypted && panel.key.passphraseRemembered === 'none'" class="grid gap-1.5">
          <Label for="key-change-current" class="text-base">Current passphrase</Label>
          <Input id="key-change-current" v-model="form.current" type="password" class="text-base" autocomplete="off" />
        </div>
      </template>

      <!-- The new file's passphrase: Generate always asks; Export and Change Passphrase offer a choice. -->
      <fieldset v-if="panel.kind !== 'generate'" class="grid gap-1">
        <legend class="mb-1 text-base">{{ panel.kind === "export" ? "Passphrase of the exported file" : "New passphrase" }}</legend>
        <label v-if="panel.kind === 'export'" class="flex items-center gap-2 text-base select-none">
          <input v-model="form.protection" type="radio" name="key-protection" value="same" class="size-3.5 accent-primary" />
          {{ panel.key.encrypted ? "The same passphrase as now" : "None, as now" }}
        </label>
        <label class="flex items-center gap-2 text-base select-none">
          <input v-model="form.protection" type="radio" name="key-protection" value="new" class="size-3.5 accent-primary" />
          A new passphrase
        </label>
        <label v-if="panel.kind === 'changePassphrase' || panel.key.encrypted" class="flex items-center gap-2 text-base select-none">
          <input v-model="form.protection" type="radio" name="key-protection" value="none" class="size-3.5 accent-primary" />
          No passphrase (anyone who can read the file can use the key)
        </label>
      </fieldset>
      <div v-if="panel.kind === 'generate' || form.protection === 'new'" class="grid grid-cols-2 gap-3">
        <div class="grid gap-1.5">
          <Label for="key-form-passphrase" class="text-base">
            Passphrase <span v-if="panel.kind === 'generate'" class="text-muted-foreground">(optional)</span>
          </Label>
          <Input id="key-form-passphrase" v-model="form.passphrase" type="password" class="text-base" autocomplete="new-password" />
        </div>
        <div class="grid gap-1.5">
          <Label for="key-form-passphrase-again" class="text-base">Passphrase again</Label>
          <Input
            id="key-form-passphrase-again"
            v-model="form.passphraseAgain"
            type="password"
            class="text-base"
            autocomplete="new-password"
            :aria-invalid="passphraseMismatch || undefined"
          />
        </div>
        <p v-if="passphraseMismatch && form.passphraseAgain.length > 0" class="col-span-2 text-xs text-destructive">The two passphrases differ.</p>
      </div>

      <p v-if="formError" class="rounded-md border border-destructive/40 bg-destructive/10 px-2.5 py-1.5 text-base break-words text-destructive">
        {{ formError }}
      </p>

      <div class="flex justify-end gap-2">
        <Button type="button" variant="outline" @click="panel = null">Cancel</Button>
        <Button type="submit" :disabled="!formReady">
          <LoaderCircleIcon v-if="formBusy" class="animate-spin" />
          {{ panel.kind === "generate" ? "Generate" : panel.kind === "export" ? "Export As…" : "Change Passphrase" }}
        </Button>
      </div>
    </form>

    <div v-if="keys === null" class="flex items-center gap-2 text-base text-muted-foreground">
      <LoaderCircleIcon class="size-3.5 animate-spin" />
      Looking for keys…
    </div>
    <p v-else-if="keys.length === 0" class="text-base text-muted-foreground">No private keys were found. Generate one, or import one you have.</p>
    <div v-else class="grid gap-2">
      <KeyRow
        v-for="k in keys"
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
  </div>
</template>
