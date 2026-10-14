<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { LoaderCircleIcon } from "@lucide/vue";
import { commands, type KeychainStatus, type PassphraseMemory } from "@/lib/bindings";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { errorMessage, IpcError, unwrap } from "@/lib/ipc";
import { keychainStatus } from "@/lib/keys";
import { useDialogsStore } from "@/stores/dialogs";

const dialogs = useDialogsStore();

const request = computed(() => dialogs.current("passphrase"));

const passphrase = ref("");
const remember = ref<PassphraseMemory>("none");
const keychain = ref<KeychainStatus | null>(null);
/** The passphrase given in this dialog was wrong; `request.wrong` covers one given before it opened. */
const wrong = ref(false);
const error = ref<string | null>(null);
const submitting = ref(false);

watch(request, (r) => {
  if (!r) return;
  passphrase.value = "";
  remember.value = r.forConnection ? "session" : "none";
  wrong.value = r.wrong ?? false;
  error.value = null;
  submitting.value = false;
  keychainStatus()
    .then((status) => (keychain.value = status))
    .catch(() => (keychain.value = null));
});

const keychainAvailable = computed(() => keychain.value?.available ?? false);

/** Where the passphrase can be remembered. A connection needs it for as long as it may reconnect. */
const choices = computed(() => {
  const list: { value: PassphraseMemory; label: string; disabled?: boolean }[] = [];
  if (!request.value?.forConnection) list.push({ value: "none", label: "Only this time" });
  list.push({ value: "session", label: "Until StashPanda closes" });
  list.push({
    value: "keychain",
    label: keychain.value ? `In ${keychain.value.name}, so it is remembered after a restart` : "In the system keychain",
    disabled: !keychainAvailable.value,
  });
  return list;
});

const canSubmit = computed(() => passphrase.value.length > 0 && !submitting.value);

// Resolve rather than cancel: on top of the Settings dialog this closes alone.
function onOpenChange(open: boolean): void {
  if (!open) dialogs.resolve("passphrase", null);
}

async function submit(): Promise<void> {
  const r = request.value;
  if (!r || !canSubmit.value) return;
  if (r.path === null) {
    // Key text with no file yet: the caller checks the passphrase.
    dialogs.resolve("passphrase", { passphrase: passphrase.value, remembered: "none" });
    return;
  }
  submitting.value = true;
  error.value = null;
  try {
    const info = await unwrap(commands.keysUnlock(r.path, passphrase.value, remember.value));
    dialogs.resolve("passphrase", { passphrase: passphrase.value, remembered: info.passphraseRemembered });
  } catch (e) {
    if (e instanceof IpcError && e.error.kind === "sshWrongPassphrase") {
      wrong.value = true;
      passphrase.value = "";
    } else {
      error.value = errorMessage(e);
    }
  } finally {
    submitting.value = false;
  }
}
</script>

<template>
  <Dialog :open="request !== null" @update:open="onOpenChange">
    <DialogContent v-if="request" class="sm:max-w-lg" size-key="passphrase">
      <DialogHeader>
        <DialogTitle>Passphrase for {{ request.keyName }}</DialogTitle>
        <DialogDescription>
          StashPanda needs the passphrase {{ request.purpose }}.
          <span v-if="request.path" class="mt-1 block truncate font-mono text-xs" :title="request.path">{{ request.path }}</span>
        </DialogDescription>
      </DialogHeader>

      <form class="grid gap-3" @submit.prevent="submit">
        <div class="grid gap-1.5">
          <Label for="passphrase-input" class="text-base">Passphrase</Label>
          <Input
            id="passphrase-input"
            v-model="passphrase"
            type="password"
            class="text-base"
            autocomplete="off"
            autofocus
            :aria-invalid="wrong || undefined"
          />
          <p v-if="wrong" class="text-xs text-destructive">That passphrase is wrong. Try again.</p>
        </div>

        <fieldset v-if="request.path" class="grid gap-1">
          <legend class="mb-1 text-base">Remember it</legend>
          <label
            v-for="choice in choices"
            :key="choice.value"
            class="flex items-center gap-2 text-base select-none"
            :class="choice.disabled && 'text-muted-foreground'"
          >
            <input v-model="remember" type="radio" name="passphrase-remember" class="size-3.5 accent-primary" :value="choice.value" :disabled="choice.disabled" />
            {{ choice.label }}
          </label>
          <!-- The keychain's own error is for diagnosing, so it is the tooltip rather than the text. -->
          <p v-if="keychain && !keychain.available" class="text-xs text-muted-foreground" :title="keychain.detail ?? undefined">
            No system keychain is available here, so a passphrase can only be remembered until StashPanda closes.
          </p>
          <p v-if="request.forConnection" class="text-xs text-muted-foreground">
            The connection needs it again if it drops and reconnects, so it is kept at least until StashPanda closes.
          </p>
        </fieldset>

        <p v-if="error" class="rounded-md border border-destructive/40 bg-destructive/10 px-2.5 py-1.5 text-base break-words text-destructive">
          {{ error }}
        </p>
      </form>

      <DialogFooter>
        <Button type="button" variant="outline" @click="dialogs.resolve('passphrase', null)">Cancel</Button>
        <Button type="button" :disabled="!canSubmit" @click="submit">
          <LoaderCircleIcon v-if="submitting" class="animate-spin" />
          Unlock
        </Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
</template>
