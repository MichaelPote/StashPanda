/**
 * Mouse wheel scrolling on Windows, at about half of WebView2's step.
 *
 * WebView2 (Chromium) scrolls 100px for each notch of a mouse wheel, where Windows' own apps and
 * Firefox move about 50–60px, so lists and diffs jump further than the rest of the desktop.
 * Neither WebView2 nor Tauri can change the step, and the wheel never reaches the Rust side, so
 * this listener takes over notched wheel scrolling in the page: it finds the element the browser
 * would have scrolled and scrolls it by `WHEEL_SCALE` of the browser's distance, animated.
 *
 * Left to the browser: macOS and Linux; Ctrl+wheel (zoom); sideways scrolling (Shift+wheel, tilt
 * wheels, touchpads); precision touchpads and free-spinning wheels, which send fractions of a
 * notch; and anything inside an element marked `data-native-wheel`, which handles the wheel
 * itself (the custom scrollbar of `ui/scroll-area`, for one).
 */

/** The share of WebView2's 100px a notch scrolls: 0.55 is about 55px. */
const WHEEL_SCALE = 0.55;

/** Smaller steps come from touchpads and high-resolution wheels, which scroll well already. */
const MIN_NOTCH_PX = 50;

/** Animate each step. With `false` (or Windows' animation effects turned off), steps jump. */
const SMOOTH = true;

/**
 * Notches closer together than this add to the position still being scrolled to, so a quick
 * run of notches covers its full distance instead of each one starting from wherever the
 * animation had got to. Longer than the browser's animation of a step.
 */
const RUN_GAP_MS = 400;

/** Where a run of notches is taking a scroller. */
interface Run {
  top: number;
  dir: number;
  at: number;
}

const runs = new WeakMap<Element, Run>();

/** The position `el` is on its way to: the current run's end, or where it is now. */
function position(el: Element, dir: number, now: number): number {
  const run = runs.get(el);
  return run && run.dir === dir && now - run.at < RUN_GAP_MS ? run.top : el.scrollTop;
}

/** Whether `el` has room to scroll further in direction `dir` (-1 up, 1 down). */
function hasRoom(el: Element, dir: number, now: number): boolean {
  const max = el.scrollHeight - el.clientHeight;
  if (max < 1) return false;
  const top = position(el, dir, now);
  return dir < 0 ? top > 0.5 : top < max - 1;
}

function scrollsY(el: Element): boolean {
  const overflow = getComputedStyle(el).overflowY;
  return overflow === "auto" || overflow === "scroll" || overflow === "overlay";
}

/**
 * The element a wheel over `start` scrolls: the nearest scrollable ancestor with room left in
 * that direction, so a nested list at its end hands on to the one around it, as the browser does.
 * The page itself only when its overflow lets it scroll: the app's root is `overflow: hidden`,
 * which code could still scroll, moving the whole interface.
 */
function scrollTarget(start: Element, dir: number, now: number): Element | null {
  for (let el: Element | null = start; el && el !== document.documentElement; el = el.parentElement) {
    if (scrollsY(el) && hasRoom(el, dir, now)) return el;
  }
  const page = document.scrollingElement;
  const pageOverflow = getComputedStyle(document.documentElement).overflowY;
  if (page && pageOverflow !== "hidden" && pageOverflow !== "clip" && hasRoom(page, dir, now)) return page;
  return null;
}

/**
 * Whether the event is whole notches of a notched wheel. Blink gives `wheelDeltaY` as 120 per
 * notch, whatever the zoom, display scaling or lines-per-notch setting; touchpads and
 * free-spinning wheels send fractions of a notch, which land between multiples of 120.
 */
function isWholeNotches(event: WheelEvent): boolean {
  const ticks = (event as WheelEvent & { wheelDeltaY?: number }).wheelDeltaY;
  return typeof ticks === "number" && ticks !== 0 && ticks % 120 === 0;
}

/** Takes over mouse wheel scrolling on Windows (WebView2); elsewhere does nothing. */
export function installWheelScroll(): void {
  if (!navigator.userAgent.includes("Windows")) return;
  const reducedMotion = window.matchMedia("(prefers-reduced-motion: reduce)");

  window.addEventListener(
    "wheel",
    (event: WheelEvent) => {
      if (event.defaultPrevented || event.ctrlKey || event.shiftKey || event.deltaX !== 0) return;
      if (event.deltaMode !== WheelEvent.DOM_DELTA_PIXEL || Math.abs(event.deltaY) < MIN_NOTCH_PX) return;
      if (!isWholeNotches(event)) return;
      const start = event.target instanceof Element ? event.target : null;
      if (!start || start.closest("[data-native-wheel]")) return;

      const dir = Math.sign(event.deltaY);
      const now = performance.now();
      const el = scrollTarget(start, dir, now);
      // Nothing can scroll that way, so the browser would not move anything either.
      if (!el) return;
      event.preventDefault();

      const max = el.scrollHeight - el.clientHeight;
      const top = Math.min(max, Math.max(0, position(el, dir, now) + event.deltaY * WHEEL_SCALE));
      runs.set(el, { top, dir, at: now });
      el.scrollTo({ top, behavior: SMOOTH && !reducedMotion.matches ? "smooth" : "instant" });
    },
    { passive: false, capture: true },
  );
}
