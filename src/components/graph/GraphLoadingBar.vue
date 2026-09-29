<script setup lang="ts">
defineProps<{
  /** Commits loaded so far. */
  loaded: number;
  /** True while a snapshot is still being built. */
  visible: boolean;
}>();
</script>

<template>
  <div v-if="visible" class="relative h-[1.2rem] shrink-0 overflow-hidden border-b border-border bg-table-head" role="status">
    <div class="absolute inset-x-0 top-0 h-[2px] overflow-hidden bg-primary/10">
      <div class="graph-loading-sweep h-full w-1/3 bg-primary/70" />
    </div>
    <p class="px-2 text-[0.7rem] leading-[1.2rem] text-muted-foreground tabular-nums">
      Loading {{ loaded.toLocaleString() }} commits…
    </p>
  </div>
</template>

<style scoped>
@keyframes graph-loading-sweep {
  from {
    transform: translateX(-100%);
  }
  to {
    transform: translateX(300%);
  }
}
.graph-loading-sweep {
  animation: graph-loading-sweep 1.2s ease-in-out infinite;
}
</style>
