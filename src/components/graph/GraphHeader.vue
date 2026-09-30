<script setup lang="ts">
import { ref } from "vue";

export type ResizableColumn = "graph" | "author" | "date" | "id";

const props = defineProps<{
  graphWidth: number;
  authorWidth: number;
  dateWidth: number;
  idWidth: number;
}>();

const emit = defineEmits<{
  /** `null` width = back to the default (for the graph column: size it to the graph). */
  resize: [column: ResizableColumn, width: number | null];
}>();

const LIMITS: Record<ResizableColumn, { min: number; max: number }> = {
  graph: { min: 28, max: 4000 },
  author: { min: 48, max: 400 },
  date: { min: 56, max: 260 },
  id: { min: 40, max: 400 },
};

/**
 * One divider per column boundary. The commit message column takes whatever is left, so the
 * divider on each side of it resizes the fixed column on the other side:
 * - after Graph: dragging right widens the graph column;
 * - before Author, Date and ID: they sit on the column's left edge, so dragging right narrows it.
 * `position` places the divider's centre on the boundary, using the column widths that
 * CommitGraphView sets as CSS variables.
 */
const DIVIDERS: { column: ResizableColumn; direction: 1 | -1; label: string; position: Record<string, string> }[] = [
  { column: "graph", direction: 1, label: "Resize the graph column", position: { left: "calc(var(--col-graph) - 3.5px)" } },
  { column: "author", direction: -1, label: "Resize the author column", position: { right: "calc(var(--col-author) + var(--col-date) + var(--col-id) - 3.5px)" } },
  { column: "date", direction: -1, label: "Resize the date column", position: { right: "calc(var(--col-date) + var(--col-id) - 3.5px)" } },
  { column: "id", direction: -1, label: "Resize the ID column", position: { right: "calc(var(--col-id) - 3.5px)" } },
];

const dragging = ref<ResizableColumn | null>(null);

function widthOf(column: ResizableColumn): number {
  switch (column) {
    case "graph":
      return props.graphWidth;
    case "author":
      return props.authorWidth;
    case "date":
      return props.dateWidth;
    case "id":
      return props.idWidth;
  }
}

function startDrag(column: ResizableColumn, direction: 1 | -1, event: PointerEvent): void {
  if (event.button !== 0) return;
  const startX = event.clientX;
  const startWidth = widthOf(column);
  const target = event.currentTarget as HTMLElement;
  target.setPointerCapture(event.pointerId);
  const { min, max } = LIMITS[column];
  dragging.value = column;

  const onMove = (e: PointerEvent): void => {
    const next = Math.round(startWidth + direction * (e.clientX - startX));
    emit("resize", column, Math.min(max, Math.max(min, next)));
  };
  const onUp = (): void => {
    dragging.value = null;
    target.removeEventListener("pointermove", onMove);
    target.removeEventListener("pointerup", onUp);
    target.removeEventListener("pointercancel", onUp);
  };
  target.addEventListener("pointermove", onMove);
  target.addEventListener("pointerup", onUp);
  target.addEventListener("pointercancel", onUp);
  event.preventDefault();
}
</script>

<template>
  <!-- The header reserves the same scrollbar space as the rows below, so the columns line up. -->
  <div
    class="graph-grid relative h-[21px] shrink-0 overflow-hidden border-b border-border bg-table-head column-head select-none scrollbar-thin [scrollbar-gutter:stable]"
  >
    <div class="flex items-center px-1.5"><span class="sr-only">Graph</span></div>
    <div class="flex min-w-0 items-center truncate px-2">Commit message</div>
    <div class="flex min-w-0 items-center truncate px-2">Author</div>
    <div class="flex min-w-0 items-center truncate px-2">Date</div>
    <div class="flex min-w-0 items-center truncate px-2">ID</div>

    <button
      v-for="d in DIVIDERS"
      :key="d.column"
      type="button"
      class="group absolute top-0 z-10 flex h-full w-[7px] cursor-col-resize justify-center outline-none"
      :style="d.position"
      :aria-label="d.label"
      :title="d.column === 'graph' ? 'Drag to set the graph width; double-click to size it to the graph' : 'Drag to set the column width; double-click for the default width'"
      @pointerdown="startDrag(d.column, d.direction, $event)"
      @dblclick="emit('resize', d.column, null)"
    >
      <span
        :class="[
          'h-full w-px transition-colors',
          dragging === d.column ? 'w-[2px] bg-primary' : 'bg-border group-hover:w-[2px] group-hover:bg-primary/70 group-focus-visible:bg-primary',
        ]"
      />
    </button>
  </div>
</template>
