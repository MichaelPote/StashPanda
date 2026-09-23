<script setup lang="ts">
import type { HTMLAttributes } from "vue";
import { cn } from "@/lib/utils";

const props = defineProps<{
  title: string;
  class?: HTMLAttributes["class"];
}>();
</script>

<template>
  <div
    :class="
      cn(
        'flex h-7 shrink-0 items-center gap-2 border-b border-border chrome px-2 select-none',
        props.class,
      )
    "
  >
    <span class="truncate text-xs font-bold tracking-[0.1em] text-muted-foreground uppercase">
      {{ title }}
    </span>
    <slot />
    <div class="ml-auto flex items-center gap-1">
      <slot name="actions" />
    </div>
  </div>
</template>
