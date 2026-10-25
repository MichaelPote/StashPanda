<script setup lang="ts">
import { computed } from "vue";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { useDialogsStore } from "@/stores/dialogs";

/** A question with several answers, as `ConfirmDialog` is one with two. */
const dialogs = useDialogsStore();

const request = computed(() => dialogs.current("choice"));

// Resolve rather than cancel: a question shown on top of another dialog closes alone.
function onOpenChange(open: boolean): void {
  if (!open) dialogs.resolve("choice", null);
}
</script>

<template>
  <Dialog :open="request !== null" @update:open="onOpenChange">
    <DialogContent v-if="request" class="sm:max-w-lg" size-key="choice">
      <DialogHeader>
        <DialogTitle>{{ request.title }}</DialogTitle>
        <DialogDescription class="whitespace-pre-line text-base">{{ request.message }}</DialogDescription>
      </DialogHeader>
      <DialogFooter>
        <Button type="button" variant="outline" @click="dialogs.resolve('choice', null)">Cancel</Button>
        <Button
          v-for="(c, i) in request.choices"
          :key="c.value"
          type="button"
          :variant="c.destructive ? 'destructive' : i === request.choices.length - 1 ? 'default' : 'outline'"
          :autofocus="i === request.choices.length - 1"
          @click="dialogs.resolve('choice', c.value)"
        >
          {{ c.label }}
        </Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
</template>
