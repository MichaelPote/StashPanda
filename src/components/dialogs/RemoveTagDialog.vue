<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import RemoteTagOptions from "@/components/dialogs/RemoteTagOptions.vue";
import { type RemoteTagChoice, useDialogsStore } from "@/stores/dialogs";
import { useSessionsStore } from "@/stores/sessions";

/** Confirms removing a tag, and asks which remotes to remove it from as well. `removeTag` does the work. */
const dialogs = useDialogsStore();
const sessions = useSessionsStore();

const request = computed(() => dialogs.current("removeTag"));
const remotes = ref<RemoteTagChoice[]>([]);

watch(request, () => {
  remotes.value = [];
});

/** The commit the tag is on, for comparing with the remotes' copies. */
const commit = computed(() => {
  const r = request.value;
  if (!r) return "";
  const entry = sessions.sessions.get(r.key)?.refs?.refs.find((e) => e.kind.type === "tag" && e.short === r.name);
  return entry?.target ?? "";
});

function onOpenChange(open: boolean): void {
  if (!open) dialogs.cancel();
}
</script>

<template>
  <Dialog :open="request !== null" @update:open="onOpenChange">
    <DialogContent v-if="request" class="sm:max-w-lg" size-key="removeTag">
      <DialogHeader>
        <DialogTitle>Remove tag {{ request.name }}?</DialogTitle>
        <DialogDescription class="text-base">
          The tag <span class="font-mono">{{ request.name }}</span> is deleted from this repository. On a remote ticked below it is
          deleted for everyone who uses that remote, though clones that have already fetched it keep their copy.
        </DialogDescription>
      </DialogHeader>

      <RemoteTagOptions v-model="remotes" :session-key="request.key" action="remove" :tag="request.name" :commit="commit" />

      <DialogFooter>
        <Button type="button" variant="outline" @click="dialogs.cancel()">Cancel</Button>
        <Button type="button" variant="destructive" autofocus @click="dialogs.resolve('removeTag', { remotes })">
          Remove tag
        </Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
</template>
