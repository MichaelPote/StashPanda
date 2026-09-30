<script setup lang="ts">
import { computed } from "vue";
import { laneVars } from "@/lib/graph/paths";

const props = defineProps<{
  /** Lane colour of the ref's tip in the graph, or null when its commit is not in the graph. */
  colour: number | null;
  /** False when the ref is switched off in the graph. */
  visible: boolean;
  name: string;
}>();

const emit = defineEmits<{
  toggle: [];
}>();

const style = computed(() => (props.colour !== null ? laneVars(props.colour) : undefined));
const title = computed(() =>
  props.visible ? `Hide ${props.name} and the commits only it reaches in the graph` : `Show ${props.name} in the graph`,
);
</script>

<template>
  <!-- An arrow-shaped tag in the ref's lane colour: filled while shown, an outline while hidden. -->
  <button
    type="button"
    class="inline-flex h-3 w-[15px] shrink-0 items-center justify-center rounded-[2px]"
    :style="style"
    :title="title"
    :aria-label="title"
    :aria-pressed="visible"
    @click.stop="emit('toggle')"
    @dblclick.stop
    @mousedown.stop
  >
    <svg viewBox="0 0 15 12" class="h-3 w-[15px]" aria-hidden="true">
      <path
        d="M1.5 1.5 H9.5 L13.5 6 L9.5 10.5 H1.5 Z"
        stroke-linejoin="round"
        :class="
          visible
            ? colour !== null
              ? 'fill-(--lane) stroke-(--lane)'
              : 'fill-muted-foreground/50 stroke-muted-foreground/50'
            : 'fill-none stroke-muted-foreground'
        "
        stroke-width="1"
      />
    </svg>
  </button>
</template>
