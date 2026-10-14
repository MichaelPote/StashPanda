<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, watch } from "vue";
import { join } from "@tauri-apps/api/path";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import type { UnlistenFn } from "@tauri-apps/api/event";
import { open as openDialog, save as saveDialog } from "@tauri-apps/plugin-dialog";
import { FileKeyIcon, FolderOpenIcon, LoaderCircleIcon } from "@lucide/vue";
import { commands, type KeyInfo } from "@/lib/bindings";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Textarea } from "@/components/ui/textarea";
import { pathBasename } from "@/lib/format";
import { errorMessage, IpcError, unwrap } from "@/lib/ipc";
import { cn } from "@/lib/utils";
import { useDialogsStore } from "@/stores/dialogs";

const props = defineProps<{
  /** The folder a new key file goes in unless the user picks another: the first scanned folder. */
  folder: string | null;
}>();

const emit = defineEmits<{
  imported: [info: KeyInfo];
  cancel: [];
}>();

const dialogs = useDialogsStore();

const text = ref("");
/** The new file's name, from a dropped or chosen file's name; `imported_key` for pasted text. */
const name = ref("imported_key");
const outPath = ref("");
/** Set once the user edits or browses for the path, so a new name no longer rewrites it. */
const outPathEdited = ref(false);
const dragging = ref(false);
const error = ref<string | null>(null);
const importing = ref(false);

/** `id_rsa.ppk` gives `id_rsa`: the import is always written in OpenSSH format. */
function stemOf(fileName: string): string {
  return fileName.replace(/\.(ppk|pem|key|txt)$/i, "") || "imported_key";
}

watch(
  [name, () => props.folder],
  async ([n, folder]) => {
    if (outPathEdited.value) return;
    outPath.value = folder ? await join(folder, n).catch(() => n) : n;
  },
  { immediate: true },
);

/** Reads a key file's text into the box. The backend refuses anything that is not a private key. */
async function readFile(path: string): Promise<void> {
  error.value = null;
  try {
    text.value = await unwrap(commands.keysReadFile(path));
    name.value = stemOf(pathBasename(path));
  } catch (e) {
    error.value = errorMessage(e);
  }
}

async function chooseFile(): Promise<void> {
  try {
    const picked = await openDialog({ directory: false, multiple: false, title: "Choose a key file to import" });
    if (typeof picked === "string" && picked.length > 0) await readFile(picked);
  } catch (e) {
    error.value = errorMessage(e);
  }
}

async function browseOutPath(): Promise<void> {
  try {
    const picked = await saveDialog({ title: "Save the imported key as", defaultPath: outPath.value || undefined });
    if (typeof picked === "string" && picked.length > 0) {
      outPath.value = picked;
      outPathEdited.value = true;
    }
  } catch (e) {
    error.value = errorMessage(e);
  }
}

// Files dragged from the desktop arrive as Tauri drag-and-drop events with their paths; the page's
// own drop events never see them. While this panel is shown, a file dropped anywhere on the
// window is imported.
let unlistenDrop: UnlistenFn | null = null;
onMounted(async () => {
  try {
    unlistenDrop = await getCurrentWebview().onDragDropEvent((event) => {
      const p = event.payload;
      if (p.type === "enter" || p.type === "over") dragging.value = true;
      else dragging.value = false;
      if (p.type === "drop" && p.paths.length > 0) void readFile(p.paths[0] as string);
    });
  } catch {
    // Not running inside Tauri (a plain browser preview): only paste and Choose File work.
  }
});
onBeforeUnmount(() => unlistenDrop?.());

const canImport = computed(() => text.value.trim().length > 0 && outPath.value.trim().length > 0 && !importing.value);

/**
 * Writes the key as a new OpenSSH file with a `.pub` beside it. An encrypted key asks for its
 * passphrase and stays encrypted with the same one.
 */
async function importKey(): Promise<void> {
  if (!canImport.value) return;
  importing.value = true;
  error.value = null;
  let passphrase: string | null = null;
  try {
    for (;;) {
      try {
        const info = await unwrap(commands.keysImport(text.value, outPath.value.trim(), passphrase, passphrase, false));
        emit("imported", info);
        return;
      } catch (e) {
        const kind = e instanceof IpcError ? e.error.kind : null;
        if (kind !== "sshNeedsPassphrase" && kind !== "sshWrongPassphrase") throw e;
        const result = await dialogs.open("passphrase", {
          path: null,
          keyName: name.value,
          purpose: "to read the key and import it",
          wrong: kind === "sshWrongPassphrase",
        });
        if (!result) return;
        passphrase = result.passphrase;
      }
    }
  } catch (e) {
    error.value = errorMessage(e);
  } finally {
    importing.value = false;
  }
}
</script>

<template>
  <div class="grid gap-3 rounded-lg border border-border p-3">
    <div
      :class="
        cn(
          'flex items-center gap-3 rounded-lg border border-dashed px-3 py-3 text-base text-muted-foreground',
          dragging ? 'border-primary text-foreground' : 'border-border',
        )
      "
    >
      <FileKeyIcon class="size-5 shrink-0" />
      <span class="min-w-0 flex-1">
        Drop a key file anywhere on this window, choose one, or paste the key's text below. PuTTY (PPK 2 and 3), PEM, OpenSSH
        and PKCS#8 keys are read.
      </span>
      <Button type="button" variant="outline" size="sm" class="shrink-0" @click="chooseFile">Choose File…</Button>
    </div>

    <div class="grid gap-1.5">
      <Label for="key-import-text" class="text-base">Key text</Label>
      <Textarea
        id="key-import-text"
        v-model="text"
        class="h-28 font-mono text-xs"
        placeholder="-----BEGIN OPENSSH PRIVATE KEY-----"
        spellcheck="false"
        autocomplete="off"
      />
    </div>

    <div class="grid gap-1.5">
      <Label for="key-import-path" class="text-base">Save as</Label>
      <div class="flex gap-2">
        <Input id="key-import-path" v-model="outPath" class="font-mono text-base" autocomplete="off" spellcheck="false" @input="outPathEdited = true" />
        <Button type="button" variant="outline" size="sm" class="h-8 shrink-0" @click="browseOutPath">
          <FolderOpenIcon />
          Browse…
        </Button>
      </div>
      <p class="text-xs text-muted-foreground">
        Saved in OpenSSH format with a .pub file beside it. An encrypted key keeps the same passphrase.
      </p>
    </div>

    <p v-if="error" class="rounded-md border border-destructive/40 bg-destructive/10 px-2.5 py-1.5 text-base break-words text-destructive">
      {{ error }}
    </p>

    <div class="flex justify-end gap-2">
      <Button type="button" variant="outline" @click="emit('cancel')">Cancel</Button>
      <Button type="button" :disabled="!canImport" @click="importKey">
        <LoaderCircleIcon v-if="importing" class="animate-spin" />
        Import
      </Button>
    </div>
  </div>
</template>
