<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { LoaderCircleIcon } from "@lucide/vue";
import { commands, type KeyInfo, type KeyPlace } from "@/lib/bindings";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { errorMessage, unwrap } from "@/lib/ipc";

/** A place a key can be copied to: Windows, a WSL distro, this computer, or a server. */
export interface KeyCopyTarget {
  id: string;
  label: string;
  place: KeyPlace;
  /** The folder the copy is written in, or null for `~/.ssh` of a WSL distro or a server, which the backend finds itself. */
  writeFolder: string | null;
}

/** The key to copy and where it can go; the dialog is open while there is one. */
export interface KeyCopyRequest {
  key: KeyInfo;
  targets: KeyCopyTarget[];
}

const props = defineProps<{ request: KeyCopyRequest | null }>();

const emit = defineEmits<{
  close: [];
  /** The copy was written: the key, the place it went to, and the path it was written as. */
  copied: [key: KeyInfo, target: KeyCopyTarget, written: string];
}>();

/** The id of the place picked, and the name to give the copy. */
const targetId = ref("");
const name = ref("");
const error = ref<string | null>(null);
const submitting = ref(false);

watch(
  () => props.request,
  (r) => {
    if (!r) return;
    targetId.value = r.targets[0]?.id ?? "";
    name.value = r.key.fileName;
    error.value = null;
    submitting.value = false;
  },
);

const target = computed(() => props.request?.targets.find((t) => t.id === targetId.value) ?? null);
const canSubmit = computed(() => target.value !== null && name.value.trim().length > 0 && !submitting.value);

// Emit rather than cancel through the dialogs store: on top of the Settings dialog this closes alone.
function onOpenChange(open: boolean): void {
  if (!open && !submitting.value) emit("close");
}

async function submit(): Promise<void> {
  const r = props.request;
  const to = target.value;
  if (!r || !to || !canSubmit.value) return;
  submitting.value = true;
  error.value = null;
  try {
    // Never overwrites: a name that is taken comes back as keyExists, and the dialog stays open for another.
    const written = await unwrap(commands.keysCopy(r.key.path, r.key.place, to.place, to.writeFolder, name.value.trim()));
    emit("copied", r.key, to, written);
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
  <Dialog :open="request !== null" @update:open="onOpenChange">
    <DialogContent v-if="request" class="sm:max-w-xl" size-key="keyCopy" data-key-copy-dialog>
      <DialogHeader>
        <DialogTitle>Copy {{ request.key.fileName }}</DialogTitle>
        <DialogDescription>
          Copies the private key{{ request.key.publicKeyFile ? " and its .pub file" : "" }} as they are, passphrase included. Into WSL or onto a
          server it goes in ~/.ssh, readable only by you. A key that is already there is never replaced: if the name is taken, choose another.
        </DialogDescription>
      </DialogHeader>

      <p v-if="request.targets.length === 0" class="text-base text-muted-foreground">
        There is nowhere else to copy it to. Open a repository on a server, or start a WSL distro, to copy it there.
      </p>
      <form v-else class="grid gap-3" @submit.prevent="submit">
        <div class="grid gap-1.5">
          <Label for="key-copy-target" class="text-base">Copy to</Label>
          <select id="key-copy-target" v-model="targetId" :class="selectClass" :disabled="submitting">
            <option v-for="t in request.targets" :key="t.id" :value="t.id">{{ t.label }}{{ t.writeFolder ? ` (${t.writeFolder})` : " (~/.ssh)" }}</option>
          </select>
        </div>
        <div class="grid gap-1.5">
          <Label for="key-copy-name" class="text-base">Name</Label>
          <Input id="key-copy-name" v-model="name" class="font-mono text-base" autocomplete="off" spellcheck="false" :disabled="submitting" />
        </div>

        <p v-if="error" class="rounded-md border border-destructive/40 bg-destructive/10 px-2.5 py-1.5 text-base break-words text-destructive">
          {{ error }}
        </p>
      </form>

      <DialogFooter>
        <Button type="button" variant="outline" :disabled="submitting" @click="emit('close')">{{ request.targets.length === 0 ? "Close" : "Cancel" }}</Button>
        <Button v-if="request.targets.length > 0" type="button" :disabled="!canSubmit" @click="submit">
          <LoaderCircleIcon v-if="submitting" class="animate-spin" />
          Copy
        </Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
</template>
