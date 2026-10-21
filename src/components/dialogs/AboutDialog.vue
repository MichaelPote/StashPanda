<script setup lang="ts">
import { computed, ref } from "vue";
import { getVersion } from "@tauri-apps/api/app";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { hostLabel } from "@/lib/format";
import { useDialogsStore } from "@/stores/dialogs";
import { useHostStore } from "@/stores/host";

/**
 * The version of the running app, as built into it from `src-tauri/tauri.conf.json`, which
 * `npm version` keeps in step with `package.json` (scripts/sync-version.mjs). Null until it has
 * been read, or when it could not be.
 */
const version = ref<string | null>(null);
getVersion().then(
  (v) => (version.value = v),
  () => (version.value = null),
);

const dialogs = useDialogsStore();
const host = useHostStore();

// The About dialog has no payload, so `current("about")` cannot tell open from closed; check the kind.
const isOpen = computed(() => dialogs.active?.kind === "about");
const hostText = computed(() => hostLabel(host.info) || "Unknown");

function onOpenChange(open: boolean): void {
  if (!open) dialogs.cancel();
}
</script>

<template>
  <Dialog :open="isOpen" @update:open="onOpenChange">
    <DialogContent class="sm:max-w-md" size-key="about">
      <DialogHeader>
        <DialogTitle>StashPanda</DialogTitle>
        <DialogDescription>A desktop Git client for local, WSL and SSH repositories.</DialogDescription>
      </DialogHeader>
      <dl class="grid grid-cols-[auto_1fr] gap-x-4 gap-y-1 text-base">
        <dt class="text-muted-foreground">Version</dt>
        <dd class="tabular-nums">{{ version ?? "Unknown" }}</dd>
        <dt class="text-muted-foreground">Host</dt>
        <dd>{{ hostText }}</dd>
      </dl>
      <DialogFooter>
        <Button type="button" @click="dialogs.resolve('about', null)">Close</Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
</template>
