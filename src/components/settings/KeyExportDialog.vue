<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { save as saveDialog } from "@tauri-apps/plugin-dialog";
import { LoaderCircleIcon } from "@lucide/vue";
import { commands, type ExportFormat, type KeyInfo } from "@/lib/bindings";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Label } from "@/components/ui/label";
import { errorMessage, unwrap } from "@/lib/ipc";
import { withPassphrase } from "@/lib/keys";
import KeyPassphraseFields from "./KeyPassphraseFields.vue";

/** The key to export; the dialog is open while there is one. */
const props = defineProps<{ keyInfo: KeyInfo | null }>();

const emit = defineEmits<{
  close: [];
  /** The export was written: the key, the path it was written as, and its format. */
  exported: [key: KeyInfo, written: string, format: ExportFormat];
}>();

const format = ref<ExportFormat>("openSsh");
/** The exported file's passphrase: the same as the key's now, none, or a new one. */
const protection = ref<"same" | "none" | "new">("same");
const passphrase = ref("");
const passphraseAgain = ref("");
const error = ref<string | null>(null);
const submitting = ref(false);

watch(
  () => props.keyInfo,
  (k) => {
    if (!k) return;
    format.value = "openSsh";
    protection.value = "same";
    passphrase.value = "";
    passphraseAgain.value = "";
    error.value = null;
    submitting.value = false;
  },
);

const canSubmit = computed(() => {
  if (submitting.value) return false;
  if (protection.value === "new") return passphrase.value.length > 0 && passphrase.value === passphraseAgain.value;
  return true;
});

/** The new passphrase as `keysConvert` takes it: null keeps the current one, "" removes it. */
function chosenPassphrase(): string | null {
  switch (protection.value) {
    case "same":
      return null;
    case "none":
      return "";
    case "new":
      return passphrase.value;
  }
}

// Emit rather than cancel through the dialogs store: on top of the Settings dialog this closes alone.
function onOpenChange(open: boolean): void {
  if (!open && !submitting.value) emit("close");
}

async function submit(): Promise<void> {
  const key = props.keyInfo;
  if (!key || !canSubmit.value) return;
  submitting.value = true;
  error.value = null;
  try {
    const chosenFormat = format.value;
    const defaultPath = key.path.replace(/\.(ppk|pem|key)$/i, "") + (chosenFormat === "pkcs8" ? ".pem" : "_openssh");
    const outPath = await saveDialog({ title: `Export ${key.fileName}`, defaultPath });
    if (typeof outPath !== "string" || outPath.length === 0) return;
    if (outPath === key.path) {
      error.value = "Choose a different file: exporting over the original key would replace it.";
      return;
    }
    const newPassphrase = chosenPassphrase();
    // Never over a file that is there, even one the save dialog offered to replace: keyExists
    // comes back instead, and the dialog stays open for another name.
    const written = await withPassphrase(key, "to export it", (pass) => unwrap(commands.keysConvert(key.path, chosenFormat, outPath, pass, newPassphrase, false)));
    if (written === undefined) return;
    emit("exported", key, written, chosenFormat);
  } catch (e) {
    error.value = errorMessage(e);
  } finally {
    submitting.value = false;
  }
}

const selectClass =
  "well h-8 w-full rounded-lg border px-2.5 text-base outline-none focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/50";
</script>

<template>
  <Dialog :open="keyInfo !== null" @update:open="onOpenChange">
    <!-- A stray click on the background would throw away a passphrase typed twice. -->
    <DialogContent v-if="keyInfo" class="sm:max-w-xl" size-key="keyExport" :close-on-outside-click="false" data-key-export-dialog>
      <DialogHeader>
        <DialogTitle>Export {{ keyInfo.fileName }}</DialogTitle>
        <DialogDescription>
          Writes a copy of the key in the format chosen, with a .pub file beside it. The original key is unchanged, and a file that is already
          there is never replaced.
        </DialogDescription>
      </DialogHeader>

      <!-- Its submit button is in the footer, tied to it by `form`: with two text fields and no submit
           button of its own, Enter would do nothing. -->
      <form id="key-export-form" class="grid gap-3" @submit.prevent="submit">
        <div class="grid gap-1.5">
          <Label for="key-export-format" class="text-base">Format</Label>
          <select id="key-export-format" v-model="format" :class="selectClass" :disabled="submitting">
            <option value="openSsh">OpenSSH (for ssh, Git and most tools)</option>
            <option value="pkcs8">PKCS#8 PEM (for tools built on OpenSSL)</option>
          </select>
        </div>

        <fieldset class="grid gap-1" :disabled="submitting">
          <legend class="mb-1 text-base">Passphrase of the exported file</legend>
          <label class="flex items-center gap-2 text-base select-none">
            <input v-model="protection" type="radio" name="key-export-protection" value="same" class="size-3.5 accent-primary" />
            {{ keyInfo.encrypted ? "The same passphrase as now" : "None, as now" }}
          </label>
          <label class="flex items-center gap-2 text-base select-none">
            <input v-model="protection" type="radio" name="key-export-protection" value="new" class="size-3.5 accent-primary" />
            A new passphrase
          </label>
          <label v-if="keyInfo.encrypted" class="flex items-center gap-2 text-base select-none">
            <input v-model="protection" type="radio" name="key-export-protection" value="none" class="size-3.5 accent-primary" />
            No passphrase (anyone who can read the file can use the key)
          </label>
        </fieldset>
        <KeyPassphraseFields
          v-if="protection === 'new'"
          v-model:passphrase="passphrase"
          v-model:again="passphraseAgain"
          id-prefix="key-export"
          :disabled="submitting"
        />

        <p v-if="error" class="rounded-md border border-destructive/40 bg-destructive/10 px-2.5 py-1.5 text-base break-words text-destructive">
          {{ error }}
        </p>
      </form>

      <DialogFooter>
        <Button type="button" variant="outline" :disabled="submitting" @click="emit('close')">Cancel</Button>
        <Button type="submit" form="key-export-form" :disabled="!canSubmit">
          <LoaderCircleIcon v-if="submitting" class="animate-spin" />
          Export As…
        </Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
</template>
