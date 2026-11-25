<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from "vue";
import { join } from "@tauri-apps/api/path";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import type { UnlistenFn } from "@tauri-apps/api/event";
import { open as openDialog, save as saveDialog } from "@tauri-apps/plugin-dialog";
import { FileKeyIcon, FolderOpenIcon, LoaderCircleIcon } from "@lucide/vue";
import { commands, type KeyInfo } from "@/lib/bindings";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Textarea } from "@/components/ui/textarea";
import { pathBasename } from "@/lib/format";
import { errorMessage, IpcError, unwrap } from "@/lib/ipc";
import { cn } from "@/lib/utils";
import { useDialogsStore } from "@/stores/dialogs";

const props = defineProps<{
  open: boolean;
  /** The folder a new key file goes in unless the user picks another: the first scanned folder. */
  folder: string | null;
}>();

const emit = defineEmits<{
  close: [];
  /** The key was written. */
  imported: [info: KeyInfo];
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

/** Offers the folder and the name as the path, until the user has chosen one. */
async function offerPath(): Promise<void> {
  if (outPathEdited.value) return;
  const n = name.value;
  outPath.value = props.folder ? await join(props.folder, n).catch(() => n) : n;
}
watch([name, () => props.folder], offerPath, { immediate: true });

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
// own drop events never see them. While this dialog is open, a file dropped anywhere on the
// window is read into it.
let unlistenDrop: UnlistenFn | null = null;
/** Counts openings and closings, so a listener that arrives after the dialog closed is removed at once. */
let listening = 0;

async function listenForDrops(): Promise<void> {
  const mine = ++listening;
  try {
    const unlisten = await getCurrentWebview().onDragDropEvent((event) => {
      const p = event.payload;
      dragging.value = p.type === "enter" || p.type === "over";
      if (p.type === "drop" && p.paths.length > 0 && !importing.value) void readFile(p.paths[0] as string);
    });
    if (mine === listening) unlistenDrop = unlisten;
    else unlisten();
  } catch {
    // Not running inside Tauri (a plain browser preview): only paste and Choose File work.
  }
}

function stopListening(): void {
  listening++;
  unlistenDrop?.();
  unlistenDrop = null;
  dragging.value = false;
}

watch(
  () => props.open,
  (open) => {
    if (!open) {
      stopListening();
      return;
    }
    text.value = "";
    name.value = "imported_key";
    outPathEdited.value = false;
    error.value = null;
    importing.value = false;
    void offerPath();
    void listenForDrops();
  },
);
onBeforeUnmount(stopListening);

const canImport = computed(() => text.value.trim().length > 0 && outPath.value.trim().length > 0 && !importing.value);

// Emit rather than cancel through the dialogs store: on top of the Settings dialog this closes alone.
function onOpenChange(open: boolean): void {
  if (!open && !importing.value) emit("close");
}

/**
 * Writes the key as a new OpenSSH file with a `.pub` beside it. An encrypted key asks for its
 * passphrase and stays encrypted with the same one.
 */
async function importKey(): Promise<void> {
  if (!props.open || !canImport.value) return;
  importing.value = true;
  error.value = null;
  let passphrase: string | null = null;
  try {
    for (;;) {
      try {
        // Never over a file that is there: keyExists comes back instead, and the dialog stays open for another name.
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
  <Dialog :open="open" @update:open="onOpenChange">
    <!-- A stray click on the background would throw away the key text pasted in. -->
    <DialogContent v-if="open" class="sm:max-w-2xl" size-key="keyImport" :close-on-outside-click="false" data-key-import-dialog>
      <DialogHeader>
        <DialogTitle>Import a key</DialogTitle>
        <DialogDescription>
          Writes the key as a new file in OpenSSH format, with a .pub file beside it. An encrypted key keeps the same passphrase, and a file
          that is already there is never replaced.
        </DialogDescription>
      </DialogHeader>

      <!-- Its submit button is in the footer, tied to it by `form`. -->
      <form id="key-import-form" class="grid gap-3" data-dialog-grow @submit.prevent="importKey">
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
            Drop a key file anywhere on this window, choose one, or paste the key's text below. PuTTY (PPK 2 and 3), PEM, OpenSSH and PKCS#8
            keys are read.
          </span>
          <Button type="button" variant="outline" size="sm" class="shrink-0" :disabled="importing" @click="chooseFile">Choose File…</Button>
        </div>

        <div class="grid gap-1.5" data-dialog-grow>
          <Label for="key-import-text" class="text-base">Key text</Label>
          <Textarea
            id="key-import-text"
            v-model="text"
            data-dialog-grow
            style="--grow-basis: 7rem"
            class="h-28 font-mono text-xs"
            placeholder="-----BEGIN OPENSSH PRIVATE KEY-----"
            spellcheck="false"
            autocomplete="off"
            :disabled="importing"
          />
        </div>

        <div class="grid gap-1.5">
          <Label for="key-import-path" class="text-base">Save as</Label>
          <div class="flex gap-2">
            <Input
              id="key-import-path"
              v-model="outPath"
              class="font-mono text-base"
              autocomplete="off"
              spellcheck="false"
              :disabled="importing"
              @input="outPathEdited = true"
            />
            <Button type="button" variant="outline" size="sm" class="h-8 shrink-0" :disabled="importing" @click="browseOutPath">
              <FolderOpenIcon />
              Browse…
            </Button>
          </div>
        </div>

        <p v-if="error" class="rounded-md border border-destructive/40 bg-destructive/10 px-2.5 py-1.5 text-base break-words text-destructive">
          {{ error }}
        </p>
      </form>

      <DialogFooter>
        <Button type="button" variant="outline" :disabled="importing" @click="emit('close')">Cancel</Button>
        <Button type="submit" form="key-import-form" :disabled="!canImport">
          <LoaderCircleIcon v-if="importing" class="animate-spin" />
          Import
        </Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
</template>
