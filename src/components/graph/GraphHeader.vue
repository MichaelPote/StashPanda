<script setup lang="ts">
export type ResizableColumn = "graph" | "author" | "date";

const props = defineProps<{
  graphWidth: number;
  authorWidth: number;
  dateWidth: number;
}>();

const emit = defineEmits<{
  /** `null` width = size the column automatically (graph column only). */
  resize: [column: ResizableColumn, width: number | null];
}>();

const LIMITS: Record<ResizableColumn, { min: number; max: number }> = {
  graph: { min: 28, max: 4000 },
  author: { min: 48, max: 400 },
  date: { min: 56, max: 260 },
};

/**
 * Each handle changes exactly one column. The graph handle grows its column when dragged right;
 * the author and date handles sit on the left edge of their column, so dragging right shrinks it.
 */
function startDrag(column: ResizableColumn, direction: 1 | -1, event: PointerEvent): void {
  const startX = event.clientX;
  const startWidth =
    column === "graph" ? props.graphWidth : column === "author" ? props.authorWidth : props.dateWidth;
  const target = event.currentTarget as HTMLElement;
  target.setPointerCapture(event.pointerId);
  const { min, max } = LIMITS[column];

  const onMove = (e: PointerEvent): void => {
    const next = Math.round(startWidth + direction * (e.clientX - startX));
    emit("resize", column, Math.min(max, Math.max(min, next)));
  };
  const onUp = (): void => {
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
  <div
    class="graph-grid h-[21px] shrink-0 border-b border-border bg-table-head column-head select-none"
  >
    <div class="relative flex items-center px-1.5">
      <span class="sr-only">Graph</span>
      <button
        type="button"
        class="absolute top-0 -right-1 z-10 h-full w-2 cursor-col-resize hover:bg-primary/30"
        aria-label="Resize graph column"
        @pointerdown="startDrag('graph', 1, $event)"
        @dblclick="emit('resize', 'graph', null)"
        title="Drag to set the graph width; double-click to size it to the graph"
      />
    </div>
    <div class="relative flex items-center truncate px-1.5">Commit message</div>
    <div class="relative flex items-center truncate px-1.5">
      <button
        type="button"
        class="absolute top-0 -left-1 z-10 h-full w-2 cursor-col-resize hover:bg-primary/30"
        aria-label="Resize author column"
        @pointerdown="startDrag('author', -1, $event)"
      />
      Author
    </div>
    <div class="relative flex items-center truncate px-1.5">
      <button
        type="button"
        class="absolute top-0 -left-1 z-10 h-full w-2 cursor-col-resize hover:bg-primary/30"
        aria-label="Resize date column"
        @pointerdown="startDrag('date', -1, $event)"
      />
      Date
    </div>
  </div>
</template>
