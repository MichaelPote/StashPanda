<script setup lang="ts">
defineProps<{
  index: number;
}>();
</script>

<template>
  <div class="graph-grid graph-row h-(--row-h)" :data-index="index" aria-hidden="true">
    <div />
    <div class="flex items-center pr-2">
      <span class="h-2.5 w-2/3 animate-pulse rounded-sm bg-muted" />
    </div>
    <div class="flex items-center px-1.5">
      <span class="h-2.5 w-1/2 animate-pulse rounded-sm bg-muted" />
    </div>
    <div class="flex items-center px-1.5">
      <span class="h-2.5 w-1/2 animate-pulse rounded-sm bg-muted" />
    </div>
  </div>
</template>
