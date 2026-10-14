import { computed, nextTick, ref, watch, type Ref, type WatchSource } from "vue";
import type { FindRange } from "@/lib/diff/find";

/** One match: the row it is in (by the view's own key), where in that row's text, and where the row sits. */
export interface FindHit {
  key: string;
  start: number;
  end: number;
  /** The row's position in what the view scrolls through, for picking the first match from the top. */
  row: number;
}

/** Past this many matches the rest are not drawn or counted; the count says so with a "+". */
export const MAX_FIND = 10_000;

export interface FindOptions<T extends FindHit> {
  /** The matches of `query` in the view's text, in order, at most `max` of them. */
  collect: (query: string, max: number) => T[];
  /** The row at the top of the view, where typing starts looking from. */
  topRow: () => number;
  /** Scrolls the view so that `hit` shows. */
  reveal: (hit: T) => void;
  /** Gives the keyboard focus back to the view when the box closes. */
  refocus: () => void;
  /** Changes after which the current match goes back to the first, without scrolling. */
  resetOn: WatchSource[];
}

/**
 * The Find box of a read-only view of lines (the diff view, the Blame dialog), as Ctrl+F opens
 * it: the text looked for, its matches and the current one, the count shown in the box, and
 * stepping through the matches with wrap-around at the ends. The view draws the matches from
 * `ranges` (see `markFound`) and puts `FindBox.vue` on screen while `open` is true.
 */
export function useFind<T extends FindHit>(options: FindOptions<T>) {
  const open = ref(false);
  const query = ref("");
  const current = ref(0);
  /** The box component, for focusing its text field. */
  const box: Ref<{ focus: () => void } | null> = ref(null);

  const matches = computed<T[]>(() => (open.value && query.value !== "" ? options.collect(query.value, MAX_FIND) : []));

  /** Each row's matches, by its key, numbered in the order the box steps through them. */
  const ranges = computed(() => {
    const map = new Map<string, FindRange[]>();
    matches.value.forEach((m, ordinal) => {
      let list = map.get(m.key);
      if (!list) map.set(m.key, (list = []));
      list.push({ start: m.start, end: m.end, ordinal });
    });
    return map;
  });

  const summary = computed(() => {
    const n = matches.value.length;
    if (query.value === "") return "";
    if (n === 0) return "No matches";
    return `${(current.value + 1).toLocaleString()} of ${n.toLocaleString()}${n >= MAX_FIND ? "+" : ""}`;
  });

  function show(): void {
    open.value = true;
    void nextTick(() => box.value?.focus());
  }

  function close(): void {
    open.value = false;
    options.refocus();
  }

  /** The next match (1) or the previous one (-1), wrapping around at the ends. */
  function step(direction: 1 | -1): void {
    const n = matches.value.length;
    if (n === 0) return;
    current.value = (current.value + direction + n) % n;
    revealCurrent();
  }

  function revealCurrent(): void {
    const hit = matches.value[current.value];
    if (hit) options.reveal(hit);
  }

  /** For the view's keydown handler: true when the event was Ctrl+F and the box is now open. */
  function onKeydown(event: KeyboardEvent): boolean {
    if (!(event.ctrlKey || event.metaKey) || event.altKey || event.shiftKey || (event.key !== "f" && event.key !== "F")) return false;
    // Handled here, so the Commits panel's window-wide Ctrl+F leaves it alone.
    event.preventDefault();
    show();
    return true;
  }

  // Typing picks the first match from the top of the view down, and shows it.
  watch(query, () => {
    const top = options.topRow();
    current.value = Math.max(0, matches.value.findIndex((m) => m.row >= top));
    revealCurrent();
  });

  // Another file, version or layout: start from the first match again, without scrolling.
  watch(options.resetOn, () => {
    current.value = 0;
  });

  return { open, query, current, box, matches, ranges, summary, show, close, step, onKeydown };
}
