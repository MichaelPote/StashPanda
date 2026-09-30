<script setup lang="ts">
import { computed } from "vue";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { hostLabel } from "@/lib/format";
import { useDialogsStore } from "@/stores/dialogs";
import { useHostStore } from "@/stores/host";

/** `package.json` cannot be imported from the frontend bundle, so the version is written here. */
const VERSION = "0.1.0";

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
        <dd class="tabular-nums">{{ VERSION }}</dd>
        <dt class="text-muted-foreground">Host</dt>
        <dd>{{ hostText }}</dd>
      </dl>
      <DialogFooter>
        <Button type="button" @click="dialogs.resolve('about', null)">Close</Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
</template>
