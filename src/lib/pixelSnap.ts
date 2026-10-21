import type { Directive } from "vue";

/**
 * `v-pixel-snap`: moves an element by the fraction of a screen pixel it landed at, so it and
 * everything inside it start on whole screen pixels (`devicePixelRatio` of them per CSS pixel,
 * which the window's zoom and the display's scaling change). The resizable panes need it: the splitter sizes them in
 * percentages, so a pane after the first usually starts part-way through a pixel, and the 12px
 * icons inside it then draw each line as two grey half-lines (see `svg.lucide.size-3\.5` in
 * index.css). Text is snapped to whole pixels by the browser and is not affected.
 *
 * The move is a plain 2D `translate` of at most half a pixel, which gives the element no
 * compositor layer of its own. Every snapped element is measured again whenever one of them
 * changes size (a divider dragged, the window resized), its component re-renders or one is
 * removed (a tab closed, moving the ones after it), outermost first, so a pane inside another
 * one is measured after its parent has moved.
 */
const snapped = new Set<HTMLElement>();
let scheduled = false;

function depth(el: Element): number {
  let d = 0;
  for (let e = el.parentElement; e; e = e.parentElement) d++;
  return d;
}

function snapAll(): void {
  scheduled = false;
  for (const el of [...snapped].sort((a, b) => depth(a) - depth(b))) {
    const [tx, ty] = (el.style.translate || "0px 0px").split(" ").map((v) => parseFloat(v) || 0);
    const box = el.getBoundingClientRect();
    // Where the element would be without its own move.
    const x = box.x - tx;
    const y = box.y - ty;
    const dpr = window.devicePixelRatio || 1;
    const next = `${Math.round(x * dpr) / dpr - x}px ${Math.round(y * dpr) / dpr - y}px`;
    if (el.style.translate !== next) el.style.translate = next;
  }
}

function schedule(): void {
  if (scheduled) return;
  scheduled = true;
  requestAnimationFrame(snapAll);
}

const observer = new ResizeObserver(schedule);
window.addEventListener("resize", schedule);
// A dialog opens with a short zoom-in, and a box measured while it is scaled is in the wrong place.
document.addEventListener("animationend", schedule, true);

export const vPixelSnap: Directive<HTMLElement> = {
  mounted(el) {
    snapped.add(el);
    observer.observe(el);
    schedule();
  },
  updated: schedule,
  unmounted(el) {
    snapped.delete(el);
    observer.unobserve(el);
    schedule();
  },
};
