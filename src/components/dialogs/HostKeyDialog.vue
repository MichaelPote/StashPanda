<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { LoaderCircleIcon, ShieldAlertIcon } from "@lucide/vue";
import { commands } from "@/lib/bindings";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { errorMessage, unwrap } from "@/lib/ipc";
import { useDialogsStore } from "@/stores/dialogs";

const dialogs = useDialogsStore();

const request = computed(() => dialogs.current("sshHostKey"));

const error = ref<string | null>(null);
const submitting = ref(false);

watch(request, (r) => {
  if (!r) return;
  error.value = null;
  submitting.value = false;
});

/** `host` on port 22, `host:2222` otherwise, as known_hosts files it. */
const address = computed(() => {
  const r = request.value;
  if (!r) return "";
  return r.port === 22 ? r.host : `${r.host}:${r.port}`;
});

function onOpenChange(open: boolean): void {
  if (!open) dialogs.resolve("sshHostKey", false);
}

/** Records the key the server offered in known_hosts. */
async function trust(): Promise<void> {
  const r = request.value;
  if (!r || r.changed || submitting.value) return;
  submitting.value = true;
  error.value = null;
  try {
    await unwrap(commands.sshTrustHost(r.host, r.port));
    dialogs.resolve("sshHostKey", true);
  } catch (e) {
    error.value = errorMessage(e);
  } finally {
    submitting.value = false;
  }
}
</script>

<template>
  <Dialog :open="request !== null" @update:open="onOpenChange">
    <DialogContent v-if="request" class="sm:max-w-xl" size-key="sshHostKey">
      <DialogHeader>
        <DialogTitle v-if="request.changed" class="flex items-center gap-2 text-destructive">
          <ShieldAlertIcon class="size-4.5 shrink-0" />
          The host key of {{ address }} has changed
        </DialogTitle>
        <DialogTitle v-else>Trust {{ address }}?</DialogTitle>
        <DialogDescription v-if="request.changed">
          StashPanda stopped connecting while opening {{ request.subject }}. The server sent a different key from the one recorded for it
          in your known_hosts file. The server may have been reinstalled, or someone may be intercepting the connection.
        </DialogDescription>
        <DialogDescription v-else>
          StashPanda has not connected to this server before, so it cannot tell whether the server is the one you mean. Check the
          fingerprint with whoever runs the server before you trust it.
        </DialogDescription>
      </DialogHeader>

      <div class="grid gap-1.5">
        <p class="text-base">{{ request.changed ? "Fingerprint of the key it sent now" : "Fingerprint of the server's key" }}</p>
        <p class="rounded-md border border-border well px-2.5 py-1.5 font-mono text-base break-all select-text">{{ request.fingerprint }}</p>
        <p v-if="request.changed" class="text-base">
          If you know why the key changed, remove the old line for {{ address }} from known_hosts (for example with
          <span class="font-mono">ssh-keygen -R</span>), then open the repository again. StashPanda does not do this for you.
        </p>
        <p v-else class="text-xs text-muted-foreground">Trusting it adds the key to your known_hosts file, so later connections check against it.</p>
      </div>

      <p v-if="error" class="rounded-md border border-destructive/40 bg-destructive/10 px-2.5 py-1.5 text-base break-words text-destructive">
        {{ error }}
      </p>

      <DialogFooter>
        <template v-if="request.changed">
          <Button type="button" autofocus @click="dialogs.resolve('sshHostKey', false)">Close</Button>
        </template>
        <template v-else>
          <Button type="button" variant="outline" @click="dialogs.resolve('sshHostKey', false)">Cancel</Button>
          <Button type="button" :disabled="submitting" @click="trust">
            <LoaderCircleIcon v-if="submitting" class="animate-spin" />
            Trust and Connect
          </Button>
        </template>
      </DialogFooter>
    </DialogContent>
  </Dialog>
</template>
