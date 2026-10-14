<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { LoaderCircleIcon } from "@lucide/vue";
import { commands } from "@/lib/bindings";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { errorMessage, unwrap } from "@/lib/ipc";
import { useDialogsStore } from "@/stores/dialogs";

const dialogs = useDialogsStore();

const request = computed(() => dialogs.current("sshPassword"));

const password = ref("");
const error = ref<string | null>(null);
const submitting = ref(false);

watch(request, (r) => {
  if (!r) return;
  password.value = "";
  error.value = null;
  submitting.value = false;
});

/** `user@host`, with the port when it is not 22. */
const login = computed(() => {
  const r = request.value;
  if (!r) return "";
  return r.port === 22 ? `${r.user}@${r.host}` : `${r.user}@${r.host}:${r.port}`;
});

const canSubmit = computed(() => password.value.length > 0 && !submitting.value);

function onOpenChange(open: boolean): void {
  if (!open) dialogs.resolve("sshPassword", false);
}

/** Hands the password to the backend for this session; the caller opens the repository again. */
async function submit(): Promise<void> {
  const r = request.value;
  if (!r || !canSubmit.value) return;
  submitting.value = true;
  error.value = null;
  try {
    await unwrap(commands.sshSetPassword(r.host, r.port, r.user, password.value));
    dialogs.resolve("sshPassword", true);
  } catch (e) {
    error.value = errorMessage(e);
  } finally {
    submitting.value = false;
  }
}
</script>

<template>
  <Dialog :open="request !== null" @update:open="onOpenChange">
    <DialogContent v-if="request" class="sm:max-w-lg" size-key="sshPassword">
      <DialogHeader>
        <DialogTitle>Password for {{ login }}</DialogTitle>
        <DialogDescription>
          The server did not accept any of your SSH keys while opening {{ request.subject }}. It accepts a password instead. StashPanda
          remembers the password until it closes, and never writes it to disk.
        </DialogDescription>
      </DialogHeader>

      <form class="grid gap-3" @submit.prevent="submit">
        <div class="grid gap-1.5">
          <Label for="ssh-password-input" class="text-base">Password</Label>
          <Input
            id="ssh-password-input"
            v-model="password"
            type="password"
            class="text-base"
            autocomplete="off"
            autofocus
            :aria-invalid="request.rejected || undefined"
          />
          <p v-if="request.rejected" class="text-xs text-destructive">The server refused the last password. Try again.</p>
        </div>

        <p v-if="error" class="rounded-md border border-destructive/40 bg-destructive/10 px-2.5 py-1.5 text-base break-words text-destructive">
          {{ error }}
        </p>
      </form>

      <DialogFooter>
        <Button type="button" variant="outline" @click="dialogs.resolve('sshPassword', false)">Cancel</Button>
        <Button type="button" :disabled="!canSubmit" @click="submit">
          <LoaderCircleIcon v-if="submitting" class="animate-spin" />
          Connect
        </Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
</template>
