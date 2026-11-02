<script setup lang="ts">
import { computed } from "vue";
import { ShoppingBagIcon } from "@lucide/vue";
import type { StashEntry } from "@/lib/bindings";
import { Tip } from "@/components/ui/tooltip";
import { laneVars } from "@/lib/graph/paths";
import { stashParts, stashTitle } from "@/lib/stash";

// The root is the tooltip, which would drop the attributes a parent passes (the context menu's
// trigger), so they go on the button itself.
defineOptions({ inheritAttrs: false });

/** Longest stash message a pill shows before it is cut short; the tooltip has all of it. */
const MAX_TEXT = 40;

/** A stash on the commit it was made on: drawn like a tag, with the Stashes heading's icon. */
const props = defineProps<{
  stash: StashEntry;
  /** Palette index: the lane colour of the commit's row. */
  colour: number;
}>();

const text = computed(() => {
  const full = stashParts(props.stash).text;
  return full.length > MAX_TEXT ? `${full.slice(0, MAX_TEXT - 1).trimEnd()}…` : full;
});
const tip = computed(() => {
  const [heading = "", ...rest] = stashTitle(props.stash).split("\n");
  return { heading, content: rest.join("\n") };
});
</script>

<template>
  <Tip :content="tip.content" :heading="tip.heading">
    <button
      v-bind="$attrs"
      type="button"
      class="pill-flat pill-lane-solid inline-flex shrink-0 items-center gap-1 px-1.5 text-sm leading-none font-normal whitespace-nowrap"
      :style="laneVars(colour)"
      data-stash-pill
    >
      <ShoppingBagIcon class="size-3 shrink-0" />
      <span class="pill-text">{{ text }}</span>
    </button>
  </Tip>
</template>
