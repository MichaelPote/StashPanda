import { onBeforeUnmount, shallowRef, watch, type Ref } from "vue";
import { useResizeObserver, useScroll } from "@vueuse/core";

export interface VirtualRowsOptions {
  rowHeight: number;
  /** Number of rows; read on every recompute. */
  total: () => number;
  /** Rows rendered above and below the viewport. */
  overscan?: number;
}

/**
 * Fixed-height row virtualisation over a scrolling container. `start`/`end` describe the rows to
 * render (end exclusive) including overscan; the caller draws a spacer of `total * rowHeight` and
 * translates one block by `start * rowHeight`. Recomputed in `requestAnimationFrame`.
 */
export function useVirtualRows(container: Ref<HTMLElement | null>, options: VirtualRowsOptions) {
  const overscan = options.overscan ?? 10;
  const H = options.rowHeight;

  const start = shallowRef(0);
  const end = shallowRef(0);
  const scrollTop = shallowRef(0);
  const viewportHeight = shallowRef(0);
  let frame = 0;

  function recompute(): void {
    const total = options.total();
    const top = scrollTop.value;
    const s = Math.max(0, Math.floor(top / H) - overscan);
    const e = Math.min(total, Math.ceil((top + viewportHeight.value) / H) + overscan);
    start.value = Math.min(s, Math.max(0, total));
    end.value = Math.max(start.value, e);
  }

  function schedule(): void {
    if (frame !== 0) return;
    frame = requestAnimationFrame(() => {
      frame = 0;
      recompute();
    });
  }

  const { y } = useScroll(container, { eventListenerOptions: { passive: true } });
  watch(y, (value) => {
    scrollTop.value = value;
    schedule();
  });

  useResizeObserver(container, (entries) => {
    const entry = entries[0];
    if (!entry) return;
    viewportHeight.value = entry.contentRect.height;
    schedule();
  });

  watch(
    () => options.total(),
    () => {
      // Read the real offset: a shrinking spacer may have clamped it.
      const el = container.value;
      if (el) scrollTop.value = el.scrollTop;
      schedule();
    },
  );

  watch(container, (el) => {
    if (!el) return;
    viewportHeight.value = el.clientHeight;
    scrollTop.value = el.scrollTop;
    schedule();
  }, { immediate: true });

  onBeforeUnmount(() => {
    if (frame !== 0) cancelAnimationFrame(frame);
  });

  function scrollTo(top: number): void {
    const el = container.value;
    if (!el) return;
    el.scrollTop = top;
    scrollTop.value = el.scrollTop;
    schedule();
  }

  /** Scrolls the least distance that brings row `index` fully into the viewport. */
  function ensureVisible(index: number): void {
    const el = container.value;
    if (!el) return;
    const top = index * H;
    const bottom = top + H;
    const viewTop = el.scrollTop;
    const viewBottom = viewTop + el.clientHeight;
    if (top < viewTop) scrollTo(top);
    else if (bottom > viewBottom) scrollTo(bottom - el.clientHeight);
  }

  /** Rows that fit in the viewport, at least one; used for PageUp/PageDown. */
  function rowsPerPage(): number {
    return Math.max(1, Math.floor(viewportHeight.value / H));
  }

  return { start, end, scrollTop, viewportHeight, scrollTo, ensureVisible, rowsPerPage, recompute: schedule };
}
