<script setup lang="ts">
import { computed } from "vue";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { useDialogsStore } from "@/stores/dialogs";

const dialogs = useDialogsStore();

const request = computed(() => dialogs.current("confirm"));

function onOpenChange(open: boolean): void {
  if (!open) dialogs.cancel();
}
</script>

<template>
  <Dialog :open="request !== null" @update:open="onOpenChange">
    <DialogContent v-if="request" class="sm:max-w-md">
      <DialogHeader>
        <DialogTitle>{{ request.title }}</DialogTitle>
        <DialogDescription class="whitespace-pre-line text-base">{{ request.message }}</DialogDescription>
      </DialogHeader>
      <DialogFooter>
        <Button type="button" variant="outline" size="sm" @click="dialogs.resolve('confirm', false)">Cancel</Button>
        <Button
          type="button"
          :variant="request.destructive ? 'destructive' : 'default'"
          size="sm"
          autofocus
          @click="dialogs.resolve('confirm', true)"
        >
          {{ request.confirmLabel ?? "Confirm" }}
        </Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
</template>
