<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { LoaderCircleIcon } from "@lucide/vue";
import { commands, type PassphraseMemory } from "@/lib/bindings";
import { Button } from "@/components/ui/button";
import { Tip } from "@/components/ui/tooltip";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { errorMessage, IpcError, unwrap } from "@/lib/ipc";
import { useDialogsStore } from "@/stores/dialogs";
import PassphraseMemoryChoice from "./PassphraseMemoryChoice.vue";

const dialogs = useDialogsStore();

const request = computed(() => dialogs.current("passphrase"));

const passphrase = ref("");
const remember = ref<PassphraseMemory>("none");
/** The passphrase given in this dialog was wrong; `request.wrong` covers one given before it opened. */
const wrong = ref(false);
const error = ref<string | null>(null);
const submitting = ref(false);

watch(request, (r) => {
  if (!r) return;
  passphrase.value = "";
  remember.value = r.remember ?? (r.forConnection ? "session" : "none");
  wrong.value = r.wrong ?? false;
  error.value = null;
  submitting.value = false;
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
    // Key text with no file yet: the caller checks the passphrase, and remembers it as chosen
    // once there is a file to remember it for.
    dialogs.resolve("passphrase", { passphrase: passphrase.value, remembered: "none", choice: r.chooseMemory ? remember.value : "none" });
    return;
  }
  submitting.value = true;
  error.value = null;
  try {
    const info = await unwrap(commands.keysUnlock(r.path, passphrase.value, remember.value));
    dialogs.resolve("passphrase", { passphrase: passphrase.value, remembered: info.passphraseRemembered, choice: remember.value });
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
          <Tip v-if="request.path" :content="request.path">
            <span class="mt-1 block truncate font-mono text-xs">{{ request.path }}</span>
          </Tip>
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

        <!-- A connection needs the passphrase for as long as it may reconnect, so "only this time" is not offered for one. -->
        <PassphraseMemoryChoice v-if="request.path || request.chooseMemory" v-model="remember" name="passphrase-remember" :allow-none="!request.forConnection">
          <p v-if="request.forConnection" class="text-xs text-muted-foreground">
            The connection needs it again if it drops and reconnects, so it is kept at least until StashPanda closes.
          </p>
        </PassphraseMemoryChoice>

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
