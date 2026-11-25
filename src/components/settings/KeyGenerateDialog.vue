<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { join } from "@tauri-apps/api/path";
import { save as saveDialog } from "@tauri-apps/plugin-dialog";
import { LoaderCircleIcon } from "@lucide/vue";
import { commands, type GenerateAlgorithm, type KeyInfo, type PassphraseMemory } from "@/lib/bindings";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { errorMessage, unwrap } from "@/lib/ipc";
import KeyPassphraseFields from "./KeyPassphraseFields.vue";
import PassphraseMemoryChoice from "./PassphraseMemoryChoice.vue";

const props = defineProps<{
  open: boolean;
  /** The folder the new key goes in unless the user picks another: the first scanned folder. */
  folder: string | null;
  /** The file names of the keys already in that folder, so the name offered is a free one. */
  takenNames: string[];
}>();

const emit = defineEmits<{
  close: [];
  /** The key was written. */
  generated: [info: KeyInfo];
}>();

const algorithm = ref<GenerateAlgorithm>("ed25519");
/** Kept from one opening to the next: the same person makes every key. */
const comment = ref("");
const outPath = ref("");
/** Set once the user edits or browses for the path, so another type no longer rewrites it. */
const outPathEdited = ref(false);
const passphrase = ref("");
const passphraseAgain = ref("");
/** Where the new key's passphrase is remembered, so that the first fetch with it does not ask. */
const remember = ref<PassphraseMemory>("none");
const error = ref<string | null>(null);
const submitting = ref(false);

watch(
  () => props.open,
  (open) => {
    if (!open) return;
    algorithm.value = "ed25519";
    outPath.value = "";
    outPathEdited.value = false;
    passphrase.value = "";
    passphraseAgain.value = "";
    remember.value = "none";
    error.value = null;
    submitting.value = false;
    if (comment.value === "") {
      unwrap(commands.keysDefaultComment())
        .then((c) => (comment.value = c))
        .catch(() => undefined);
    }
  },
);

/** `id_ed25519`, or `id_ed25519_2` and so on when a key in the folder has the name. */
function freeKeyName(base: string): string {
  const taken = new Set(props.takenNames);
  if (!taken.has(base)) return base;
  for (let n = 2; ; n++) if (!taken.has(`${base}_${n}`)) return `${base}_${n}`;
}

// The file name follows the type until the user edits the path.
watch(
  () => [props.open, algorithm.value, props.folder] as const,
  async ([open, chosen, folder]) => {
    if (!open || outPathEdited.value) return;
    const name = freeKeyName(chosen === "ed25519" ? "id_ed25519" : "id_rsa");
    outPath.value = folder ? await join(folder, name).catch(() => name) : name;
  },
);

/** An empty passphrase is allowed: the key is then written unencrypted. */
const canSubmit = computed(() => !submitting.value && passphrase.value === passphraseAgain.value && outPath.value.trim().length > 0);

// Emit rather than cancel through the dialogs store: on top of the Settings dialog this closes alone.
function onOpenChange(open: boolean): void {
  if (!open && !submitting.value) emit("close");
}

async function browse(): Promise<void> {
  try {
    const picked = await saveDialog({ title: "Save the new key as", defaultPath: outPath.value || undefined });
    if (typeof picked === "string" && picked.length > 0) {
      outPath.value = picked;
      outPathEdited.value = true;
    }
  } catch (e) {
    error.value = errorMessage(e);
  }
}

/**
 * Remembers the new key's passphrase as chosen, and gives the key as it then reads. The key is
 * written by now, so a keychain that refuses is not an error of this dialog: the key row shows
 * where the passphrase is remembered.
 */
async function remembered(info: KeyInfo): Promise<KeyInfo> {
  if (passphrase.value === "" || remember.value === "none") return info;
  try {
    return await unwrap(commands.keysUnlock(info.path, passphrase.value, remember.value));
  } catch {
    return info;
  }
}

async function submit(): Promise<void> {
  if (!props.open || !canSubmit.value) return;
  submitting.value = true;
  error.value = null;
  try {
    // Never over a file that is there, even one the save dialog offered to replace: keyExists
    // comes back instead, and the dialog stays open for another name.
    const info = await unwrap(commands.keysGenerate(algorithm.value, comment.value.trim(), outPath.value.trim(), passphrase.value || null, false));
    emit("generated", await remembered(info));
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
  <Dialog :open="open" @update:open="onOpenChange">
    <!-- A stray click on the background would throw away a passphrase typed twice. -->
    <DialogContent v-if="open" class="sm:max-w-xl" size-key="keyGenerate" :close-on-outside-click="false" data-key-generate-dialog>
      <DialogHeader>
        <DialogTitle>Generate a key</DialogTitle>
        <DialogDescription>
          Writes a new private key, with a .pub file beside it. A file that is already there is never replaced.
        </DialogDescription>
      </DialogHeader>

      <!-- Its submit button is in the footer, tied to it by `form`: with several text fields and no
           submit button of its own, Enter would do nothing. -->
      <form id="key-generate-form" class="grid gap-3" @submit.prevent="submit">
        <div class="grid grid-cols-2 gap-3">
          <div class="grid gap-1.5">
            <Label for="key-generate-algorithm" class="text-base">Type</Label>
            <select id="key-generate-algorithm" v-model="algorithm" :class="selectClass" :disabled="submitting">
              <option value="ed25519">Ed25519 (recommended)</option>
              <option value="rsa4096">RSA 4096, for servers that do not take Ed25519</option>
            </select>
          </div>
          <div class="grid gap-1.5">
            <Label for="key-generate-comment" class="text-base">Comment</Label>
            <Input id="key-generate-comment" v-model="comment" class="text-base" autocomplete="off" spellcheck="false" :disabled="submitting" />
          </div>
        </div>
        <div class="grid gap-1.5">
          <Label for="key-generate-path" class="text-base">Save as</Label>
          <div class="flex gap-2">
            <Input
              id="key-generate-path"
              v-model="outPath"
              class="font-mono text-base"
              autocomplete="off"
              spellcheck="false"
              :disabled="submitting"
              @input="outPathEdited = true"
            />
            <Button type="button" variant="outline" size="sm" class="h-8 shrink-0" :disabled="submitting" @click="browse">Browse…</Button>
          </div>
        </div>
        <KeyPassphraseFields v-model:passphrase="passphrase" v-model:again="passphraseAgain" id-prefix="key-generate" optional :disabled="submitting" />
        <PassphraseMemoryChoice
          v-if="passphrase.length > 0"
          v-model="remember"
          name="key-generate-remember"
          legend="Remember the passphrase"
          none-label="Do not remember it: ask when the key is used"
          :disabled="submitting"
        />

        <p v-if="error" class="rounded-md border border-destructive/40 bg-destructive/10 px-2.5 py-1.5 text-base break-words text-destructive">
          {{ error }}
        </p>
      </form>

      <DialogFooter>
        <Button type="button" variant="outline" :disabled="submitting" @click="emit('close')">Cancel</Button>
        <Button type="submit" form="key-generate-form" :disabled="!canSubmit">
          <LoaderCircleIcon v-if="submitting" class="animate-spin" />
          Generate
        </Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
</template>
