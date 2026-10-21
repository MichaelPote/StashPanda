<script setup lang="ts">
import { useCommitColumns } from "@/composables/useCommitColumns";

defineProps<{
  index: number;
}>();

/** Hidden columns keep an empty cell, at no width, so the grid's columns stay in place. */
const { shown } = useCommitColumns();
</script>

<template>
  <div class="graph-grid graph-row h-(--row-h)" :data-index="index" aria-hidden="true">
    <div />
    <div class="flex items-center pr-2">
      <span class="h-2.5 w-2/3 animate-pulse rounded-sm bg-muted" />
    </div>
    <div v-if="shown.author" class="flex items-center px-1.5">
      <span class="h-2.5 w-1/2 animate-pulse rounded-sm bg-muted" />
    </div>
    <div v-else />
    <div v-if="shown.date" class="flex items-center px-1.5">
      <span class="h-2.5 w-1/2 animate-pulse rounded-sm bg-muted" />
    </div>
    <div v-else />
    <div v-if="shown.id" class="flex items-center px-1.5">
      <span class="h-2.5 w-2/3 animate-pulse rounded-sm bg-muted" />
    </div>
    <div v-else />
  </div>
</template>
