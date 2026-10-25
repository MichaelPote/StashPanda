<script setup lang="ts">
import type { Component } from "vue";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { BUTTON_CLASS, CAPTION_CLASS, FLAT_CLASS } from "@/lib/toolbarStyle";
import { cn } from "@/lib/utils";

// Attributes such as `data-*` go on the button, not on the tooltip around it.
defineOptions({ inheritAttrs: false });

/**
 * A button for a toolbar other than the window's own, drawn the same way: icon over a caption,
 * with the tooltip the window's toolbar gives its buttons. It grows past the usual width for a
 * longer caption rather than cutting it short.
 */
const props = defineProps<{
  icon: Component;
  caption: string;
  /** The tooltip's bold first line. */
  title: string;
  /** What the button does, under the title. */
  description: string;
  /** Why the button cannot be used now; null or undefined when it can. */
  reason?: string | null;
}>();

const emit = defineEmits<{
  click: [];
}>();
</script>

<template>
  <!-- A disabled button swallows pointer events, so the wrapper span carries the tooltip. -->
  <Tooltip>
    <TooltipTrigger as-child>
      <span class="inline-flex shrink-0">
        <button
          type="button"
          :class="cn(BUTTON_CLASS, FLAT_CLASS, 'w-auto min-w-[63px] px-[6px]')"
          :aria-label="props.title"
          :disabled="!!props.reason"
          v-bind="$attrs"
          @click="emit('click')"
        >
          <component :is="props.icon" />
          <span :class="cn(CAPTION_CLASS, 'max-w-none')">{{ props.caption }}</span>
        </button>
      </span>
    </TooltipTrigger>
    <TooltipContent side="bottom" class="max-w-72">
      <div class="flex flex-col gap-0.5">
        <span class="font-semibold">{{ props.title }}</span>
        <span>{{ props.description }}</span>
        <span v-if="props.reason" class="italic">{{ props.reason }}</span>
      </div>
    </TooltipContent>
  </Tooltip>
</template>
