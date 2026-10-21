<script setup lang="ts">
import { ContextMenuItem } from "@/components/ui/context-menu";
import { Tip } from "@/components/ui/tooltip";
import { cn } from "@/lib/utils";

/**
 * A context menu item that can be disabled for a stated reason. The reason becomes the item's
 * tooltip, so hovering a greyed-out item explains why it is unavailable. shadcn's item turns
 * pointer events off when disabled, which would also stop the tooltip, so that is switched back on.
 */
const props = defineProps<{
  /** Why the item is unavailable; null or undefined means it is enabled. */
  reason?: string | null;
  destructive?: boolean;
}>();

const emit = defineEmits<{
  select: [];
}>();
</script>

<template>
  <Tip :content="props.reason" side="right">
    <ContextMenuItem
      :class="cn('text-base', props.reason && 'data-disabled:pointer-events-auto')"
      :disabled="!!props.reason"
      :variant="props.destructive ? 'destructive' : 'default'"
      @select="emit('select')"
    >
      <slot />
    </ContextMenuItem>
  </Tip>
</template>
