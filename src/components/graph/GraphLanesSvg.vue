<script setup lang="ts">
import { computed } from "vue";
import type { GraphRow } from "@/lib/bindings";
import { buildLanePaths, laneColour, LANE_WIDTH } from "@/lib/graph/paths";

const props = defineProps<{
  /** Rows from `first` onwards; `undefined` where a page is still loading. */
  rows: (GraphRow | undefined)[];
  /** Snapshot index of `rows[0]`. */
  first: number;
  /** Snapshot index of the translated block's first row (the SVG is offset by the difference). */
  blockStart: number;
  rowHeight: number;
  width: number;
}>();

const paths = computed(() => buildLanePaths(props.rows, props.first, props.rowHeight, LANE_WIDTH));
const height = computed(() => props.rows.length * props.rowHeight);
const top = computed(() => (props.first - props.blockStart) * props.rowHeight);
</script>

<template>
  <svg
    class="pointer-events-none absolute left-0"
    :style="{ top: `${top}px`, width: `${width}px`, height: `${height}px` }"
    :width="width"
    :height="height"
    :viewBox="`0 0 ${width} ${height}`"
    aria-hidden="true"
  >
    <g fill="none" stroke-width="2" stroke-linecap="round">
      <path v-for="p in paths.runs" :key="`r${p.colour}`" :d="p.d" :stroke="laneColour(p.colour)" />
      <path v-for="p in paths.curves" :key="`c${p.colour}`" :d="p.d" :stroke="laneColour(p.colour)" />
      <path
        v-for="p in paths.stubs"
        :key="`s${p.colour}`"
        :d="p.d"
        :stroke="laneColour(p.colour)"
        stroke-dasharray="3 3"
        opacity="0.4"
      />
    </g>
    <g>
      <template v-for="d in paths.dots" :key="d.index">
        <circle
          v-if="d.isWorkingTree"
          :cx="d.cx"
          :cy="d.cy"
          r="4"
          fill="var(--background)"
          :stroke="laneColour(d.colour)"
          stroke-width="1.5"
          stroke-dasharray="2 2"
        />
        <template v-else-if="d.isHead">
          <circle :cx="d.cx" :cy="d.cy" r="5" fill="var(--background)" :stroke="laneColour(d.colour)" stroke-width="2" />
          <circle :cx="d.cx" :cy="d.cy" r="2" :fill="laneColour(d.colour)" />
        </template>
        <circle v-else :cx="d.cx" :cy="d.cy" r="3.5" :fill="laneColour(d.colour)" />
      </template>
    </g>
  </svg>
</template>
