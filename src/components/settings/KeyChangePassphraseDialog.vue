<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { LoaderCircleIcon } from "@lucide/vue";
import { commands, type KeyInfo } from "@/lib/bindings";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { errorMessage, unwrap } from "@/lib/ipc";
import { withPassphrase } from "@/lib/keys";
import KeyPassphraseFields from "./KeyPassphraseFields.vue";

/** The key whose passphrase to change; the dialog is open while there is one. */
const props = defineProps<{ keyInfo: KeyInfo | null }>();

const emit = defineEmits<{
  close: [];
  /** The key file was written again: the key as it is now, and whether its passphrase was removed rather than changed. */
  changed: [info: KeyInfo, removed: boolean];
}>();

/** The key's passphrase now, asked for here when it is encrypted and nothing remembers it. */
const current = ref("");
/** A new passphrase, or none at all. */
const protection = ref<"new" | "none">("new");
const passphrase = ref("");
const passphraseAgain = ref("");
const error = ref<string | null>(null);
const submitting = ref(false);

watch(
  () => props.keyInfo,
  (k) => {
    if (!k) return;
    current.value = "";
    protection.value = "new";
    passphrase.value = "";
    passphraseAgain.value = "";
    error.value = null;
    submitting.value = false;
  },
);

const asksCurrent = computed(() => props.keyInfo !== null && props.keyInfo.encrypted && props.keyInfo.passphraseRemembered === "none");

const canSubmit = computed(() => {
  if (submitting.value) return false;
  if (protection.value === "new") return passphrase.value.length > 0 && passphrase.value === passphraseAgain.value;
  return true;
});

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
    const given = current.value || null;
    const next = protection.value === "none" ? null : passphrase.value;
    // First with the passphrase typed here, or a remembered one; the passphrase dialog asks again when that is wrong.
    const info = await withPassphrase(key, "to change it", (pass) => unwrap(commands.keysChangePassphrase(key.path, pass ?? given, next)));
    if (info === undefined) return;
    emit("changed", info, next === null);
  } catch (e) {
    error.value = errorMessage(e);
  } finally {
    submitting.value = false;
  }
}
</script>

<template>
  <Dialog :open="keyInfo !== null" @update:open="onOpenChange">
    <!-- A stray click on the background would throw away a passphrase typed twice. -->
    <DialogContent v-if="keyInfo" class="sm:max-w-xl" size-key="keyChangePassphrase" :close-on-outside-click="false" data-key-passphrase-dialog>
      <DialogHeader>
        <DialogTitle>Change the passphrase of {{ keyInfo.fileName }}</DialogTitle>
        <DialogDescription>
          Writes the key file again, encrypted with the new passphrase. The key itself and its public key stay the same, so servers that accept
          it still do.
        </DialogDescription>
      </DialogHeader>

      <!-- Its submit button is in the footer, tied to it by `form`: with two text fields and no submit
           button of its own, Enter would do nothing. -->
      <form id="key-change-form" class="grid gap-3" @submit.prevent="submit">
        <div v-if="asksCurrent" class="grid gap-1.5">
          <Label for="key-change-current" class="text-base">Current passphrase</Label>
          <Input id="key-change-current" v-model="current" type="password" class="text-base" autocomplete="off" :disabled="submitting" />
        </div>

        <fieldset class="grid gap-1" :disabled="submitting">
          <legend class="mb-1 text-base">New passphrase</legend>
          <label class="flex items-center gap-2 text-base select-none">
            <input v-model="protection" type="radio" name="key-change-protection" value="new" class="size-3.5 accent-primary" />
            A new passphrase
          </label>
          <label class="flex items-center gap-2 text-base select-none">
            <input v-model="protection" type="radio" name="key-change-protection" value="none" class="size-3.5 accent-primary" />
            No passphrase (anyone who can read the file can use the key)
          </label>
        </fieldset>
        <KeyPassphraseFields
          v-if="protection === 'new'"
          v-model:passphrase="passphrase"
          v-model:again="passphraseAgain"
          id-prefix="key-change"
          :disabled="submitting"
        />

        <p v-if="error" class="rounded-md border border-destructive/40 bg-destructive/10 px-2.5 py-1.5 text-base break-words text-destructive">
          {{ error }}
        </p>
      </form>

      <DialogFooter>
        <Button type="button" variant="outline" :disabled="submitting" @click="emit('close')">Cancel</Button>
        <Button type="submit" form="key-change-form" :disabled="!canSubmit">
          <LoaderCircleIcon v-if="submitting" class="animate-spin" />
          Change Passphrase
        </Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
</template>
