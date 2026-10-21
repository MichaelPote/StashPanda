<script setup lang="ts">
import type { HTMLAttributes } from "vue";
import { cn } from "@/lib/utils";

const props = defineProps<{
  title: string;
  class?: HTMLAttributes["class"];
}>();
</script>

<!-- Nothing in the header wraps. The title (one short word) and the actions keep their size, what
     comes between them (a path, a commit ID) truncates, and what still does not fit is cut off at
     the panel's edge. -->
<template>
  <div
    :class="
      cn(
        'flex h-[24px] shrink-0 items-center gap-2 overflow-hidden border-b border-border chrome px-2 whitespace-nowrap select-none',
        props.class,
      )
    "
  >
    <span class="shrink-0 smallcaps">
      {{ title }}
    </span>
    <slot />
    <div class="ml-auto flex shrink-0 items-center gap-1">
      <slot name="actions" />
    </div>
  </div>
</template>
