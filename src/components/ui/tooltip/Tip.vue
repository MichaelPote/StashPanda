<script setup lang="ts">
import Tooltip from "./Tooltip.vue";
import TooltipContent from "./TooltipContent.vue";
import TooltipTrigger from "./TooltipTrigger.vue";

/**
 * The app's tooltip around one element, in place of a native `title`: `content`, under a bold
 * `heading` when there is one. A line break in `content` starts a new line, and a long path or
 * commit ID wraps anywhere. With no content and no heading (an empty string, null or undefined)
 * it adds no tooltip, so one shown only sometimes, such as the reason something is unavailable,
 * needs no `v-if`.
 *
 * The slot must be a single element, which becomes the trigger. A disabled button lets no pointer
 * events through, so put a disabled one inside a span and give the span the tooltip.
 */
withDefaults(
  defineProps<{
    content?: string | null;
    heading?: string | null;
    side?: "top" | "right" | "bottom" | "left";
  }>(),
  { content: null, heading: null, side: "bottom" },
);
</script>

<template>
  <Tooltip v-if="content || heading">
    <TooltipTrigger as-child>
      <slot />
    </TooltipTrigger>
    <TooltipContent :side="side" class="max-w-sm">
      <!-- A column: the tooltip lays its content out in a row. -->
      <div class="flex flex-col gap-0.5 whitespace-pre-line [overflow-wrap:anywhere]">
        <span v-if="heading" class="font-semibold">{{ heading }}</span>
        <span v-if="content">{{ content }}</span>
      </div>
    </TooltipContent>
  </Tooltip>
  <slot v-else />
</template>
