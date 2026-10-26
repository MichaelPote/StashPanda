<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { openUrl } from "@tauri-apps/plugin-opener";
import { relaunch } from "@tauri-apps/plugin-process";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { commands } from "@/lib/bindings";
import { errorMessage, unwrap } from "@/lib/ipc";
import { RELEASES_URL } from "@/lib/updates";
import { useDialogsStore } from "@/stores/dialogs";
import { useHostStore } from "@/stores/host";

const dialogs = useDialogsStore();
const host = useHostStore();

const request = computed(() => dialogs.current("update"));

/** Bytes downloaded so far, and the whole size when the server sent it; null before the download starts. */
const received = ref<number | null>(null);
const total = ref<number | null>(null);
/** Downloaded, and the installer is running. */
const installing = ref(false);
const error = ref<string | null>(null);

const busy = computed(() => received.value !== null && error.value === null);
const percent = computed(() => (total.value ? Math.min(100, Math.round(((received.value ?? 0) / total.value) * 100)) : null));
const releaseUrl = computed(() => (request.value ? `${RELEASES_URL}/tag/v${request.value.update.version}` : RELEASES_URL));

watch(request, () => {
  received.value = null;
  total.value = null;
  installing.value = false;
  error.value = null;
});

async function install(): Promise<void> {
  const update = request.value?.update;
  if (!update) return;
  error.value = null;
  try {
    // The Windows installer ends the app at once, without the wait for Git that closing it has.
    if (!(await unwrap(commands.updatePrepare()))) {
      error.value = "Git is still changing a repository. Install the update once it has finished.";
      return;
    }
    received.value = 0;
    await update.download((event) => {
      if (event.event === "Started") total.value = event.data.contentLength ?? null;
      else if (event.event === "Progress") received.value = (received.value ?? 0) + event.data.chunkLength;
    });
    installing.value = true;
    await update.install();
    // Windows never gets here: its installer has already closed the app and starts it again.
    await relaunch();
  } catch (e) {
    error.value = `The update could not be installed: ${errorMessage(e)}`;
    installing.value = false;
  }
}

async function openRelease(): Promise<void> {
  try {
    await openUrl(releaseUrl.value);
  } catch (e) {
    error.value = `Could not open ${releaseUrl.value}: ${errorMessage(e)}`;
  }
}

function onOpenChange(open: boolean): void {
  if (!open && !busy.value) dialogs.cancel();
}
</script>

<template>
  <Dialog :open="request !== null" @update:open="onOpenChange">
    <DialogContent v-if="request" class="sm:max-w-md" size-key="update">
      <DialogHeader>
        <DialogTitle>StashPanda {{ request.update.version }} is available</DialogTitle>
        <DialogDescription class="text-base">
          You have version {{ request.update.currentVersion }}. StashPanda restarts once the new version is installed.
          <template v-if="host.info?.type === 'linux' || host.info?.type === 'linuxWsl'">
            Installed from a .deb or .rpm package, it asks for your password to install the new one.
          </template>
        </DialogDescription>
      </DialogHeader>
      <button type="button" class="self-start text-base text-primary underline-offset-4 hover:underline" @click="openRelease">
        See what's new in {{ request.update.version }}
      </button>
      <div v-if="busy" class="flex flex-col gap-1.5">
        <div class="h-1.5 overflow-hidden rounded-full bg-muted">
          <div class="h-full bg-primary transition-[width]" :style="{ width: `${installing ? 100 : (percent ?? 0)}%` }" />
        </div>
        <p class="text-sm text-muted-foreground">
          {{ installing ? "Installing…" : percent === null ? "Downloading…" : `Downloading… ${percent}%` }}
        </p>
      </div>
      <p v-if="error" class="text-base text-destructive">{{ error }}</p>
      <DialogFooter>
        <Button type="button" variant="outline" :disabled="busy" @click="dialogs.cancel()">Later</Button>
        <Button type="button" autofocus :disabled="busy" @click="install">Install and Restart</Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
</template>
