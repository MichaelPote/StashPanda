<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { useDialogsStore } from "@/stores/dialogs";

/**
 * A user name or a password that Git, or the `ssh` it started, asks for through the askpass
 * helper, in their own words. A key's passphrase has the passphrase dialog instead, which can
 * check it against the key and remember it.
 */
const dialogs = useDialogsStore();

const request = computed(() => dialogs.current("askpass"));

const text = ref("");

watch(request, (r) => {
  if (r) text.value = "";
});

const canSubmit = computed(() => text.value.length > 0);

function onOpenChange(open: boolean): void {
  if (!open) dialogs.resolve("askpass", null);
}

function submit(): void {
  if (request.value && canSubmit.value) dialogs.resolve("askpass", text.value);
}
</script>

<template>
  <Dialog :open="request !== null" @update:open="onOpenChange">
    <DialogContent v-if="request" class="sm:max-w-lg" size-key="askpass" data-askpass-dialog>
      <DialogHeader>
        <DialogTitle>{{ request.secret ? "Password needed" : "User name needed" }}</DialogTitle>
        <DialogDescription>
          git {{ request.command }} cannot go on without {{ request.secret ? "a password" : "a user name" }}. This is what it asks:
          <span class="mt-1 block font-mono text-xs break-words whitespace-pre-line">{{ request.prompt }}</span>
        </DialogDescription>
      </DialogHeader>

      <form id="askpass-form" class="grid gap-3" @submit.prevent="submit">
        <div class="grid gap-1.5">
          <Label for="askpass-input" class="text-base">{{ request.secret ? "Password" : "User name" }}</Label>
          <Input id="askpass-input" v-model="text" :type="request.secret ? 'password' : 'text'" class="text-base" autocomplete="off" spellcheck="false" autofocus />
        </div>
        <p class="text-xs text-muted-foreground">
          StashPanda passes this on to Git and does not remember it. With a credential helper set up (Git Credential Manager on Windows), Git
          remembers it and stops asking.
        </p>
      </form>

      <DialogFooter>
        <Button type="button" variant="outline" @click="dialogs.resolve('askpass', null)">Cancel</Button>
        <Button type="submit" form="askpass-form" :disabled="!canSubmit">Continue</Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
</template>
