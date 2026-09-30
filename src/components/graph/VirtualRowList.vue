<script setup lang="ts">
import { nextTick, ref, watch } from "vue";
import { useVirtualRows } from "@/composables/useVirtualRows";

const props = defineProps<{
  total: number;
  rowHeight: number;
  /** Offset to restore on mount and whenever the owner changes it (snapshot reconciliation). */
  savedScrollTop: number;
}>();

const emit = defineEmits<{
  "update:scrollTop": [top: number];
}>();

const scroller = ref<HTMLDivElement | null>(null);

const virtual = useVirtualRows(scroller, {
  rowHeight: props.rowHeight,
  total: () => props.total,
  overscan: 10,
});

watch(virtual.scrollTop, (top) => {
  if (Math.abs(top - props.savedScrollTop) > 0.5) emit("update:scrollTop", top);
});

// Apply the saved offset once the spacer is tall enough to hold it, and again whenever the owner
// moves it (the same row must stay under the cursor after a snapshot swap).
watch(
  () => [props.savedScrollTop, props.total] as const,
  async ([wanted]) => {
    await nextTick();
    const el = scroller.value;
    if (!el) return;
    if (Math.abs(el.scrollTop - wanted) > 0.5) virtual.scrollTo(wanted);
  },
  { immediate: true },
);

defineExpose({
  start: virtual.start,
  end: virtual.end,
  ensureVisible: virtual.ensureVisible,
  rowsPerPage: virtual.rowsPerPage,
  scrollTo: virtual.scrollTo,
  element: scroller,
});
</script>

<template>
  <div ref="scroller" class="relative h-full min-h-0 overflow-auto scrollbar-thin [scrollbar-gutter:stable]">
    <div class="relative w-full" :style="{ height: `${total * rowHeight}px` }">
      <div
        class="absolute inset-x-0 top-0 will-change-transform"
        :style="{ transform: `translateY(${virtual.start.value * rowHeight}px)` }"
      >
        <slot :start="virtual.start.value" :end="virtual.end.value" />
      </div>
    </div>
  </div>
</template>
