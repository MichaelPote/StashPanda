<script setup lang="ts">
import type { HTMLAttributes } from "vue";
import { cn } from "@/lib/utils";

const props = defineProps<{
  message: string;
  class?: HTMLAttributes["class"];
}>();
</script>

<template>
  <div
    :class="
      cn('flex h-full min-h-0 flex-col items-center justify-center gap-3 p-4 text-center', props.class)
    "
  >
    <p class="text-base text-muted-foreground">{{ message }}</p>
    <slot />
  </div>
</template>
