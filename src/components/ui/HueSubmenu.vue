<script setup lang="ts">
import { CheckIcon } from "@lucide/vue";
import { ContextMenuItem, ContextMenuSeparator, ContextMenuSub, ContextMenuSubContent, ContextMenuSubTrigger } from "@/components/ui/context-menu";
import { HUES, useHueNames } from "@/lib/hues";

/**
 * A context menu's "Colour" submenu: Automatic, then the palette's six hues as the graph draws
 * them (`lib/hues.ts`), with a tick on the one chosen. Used for branches, tags and repositories.
 */
defineProps<{
  /** The hue chosen now (palette index 0 to 5), or null for the automatic colour. */
  chosen: number | null;
}>();

const emit = defineEmits<{
  /** A hue, or null for Automatic. */
  choose: [colour: number | null];
}>();

const names = useHueNames();
</script>

<template>
  <ContextMenuSub>
    <ContextMenuSubTrigger class="text-base" data-colour-menu>Colour</ContextMenuSubTrigger>
    <ContextMenuSubContent class="min-w-40">
      <ContextMenuItem class="text-base" data-colour="auto" @select="emit('choose', null)">
        <span class="size-3 shrink-0 rounded-sm border border-dashed border-muted-foreground" aria-hidden="true" />
        Automatic
        <CheckIcon v-if="chosen === null" class="ml-auto size-3.5" />
      </ContextMenuItem>
      <ContextMenuSeparator />
      <ContextMenuItem v-for="hue in HUES" :key="hue" class="text-base" :data-colour="hue" @select="emit('choose', hue)">
        <span class="size-3 shrink-0 rounded-sm" :style="{ backgroundColor: `var(--graph-lane-${hue})` }" aria-hidden="true" />
        {{ names[hue] }}
        <CheckIcon v-if="chosen === hue" class="ml-auto size-3.5" />
      </ContextMenuItem>
    </ContextMenuSubContent>
  </ContextMenuSub>
</template>
