import { ref } from "vue";

/** Selected ids plus the anchor a Shift-range starts from. Replaced, never mutated. */
export interface ListSelectionState {
  ids: Set<string>;
  anchorId: string | null;
}

export interface ListSelectionOptions {
  /** Every id in display order. */
  order: () => readonly string[];
  get: () => ListSelectionState;
  set: (next: ListSelectionState) => void;
  /** Called when a click or arrow key selects an item (not when Ctrl-click deselects one). */
  onActivate?: (id: string) => void;
  /** Scrolls an item into view after keyboard movement. */
  reveal?: (id: string) => void;
  /** Where the keyboard starts when nothing has been clicked yet. */
  initialCursor?: () => string | null;
}

/**
 * Desktop list selection shared by the Files panel and the Commit dialog:
 * - click selects one item; Ctrl-click (Cmd on macOS) toggles one; Shift-click selects from the
 *   anchor to the item, and Ctrl+Shift-click adds that range to the selection;
 * - Up, Down, Home and End move and select; with Shift they extend from the anchor; with Ctrl
 *   they move the cursor only, and Ctrl+Space then toggles the item under it;
 * - Ctrl+A selects every item.
 * The cursor is the item the keyboard moves from: the last one clicked or reached by arrow key.
 */
export function useListSelection(options: ListSelectionOptions) {
  const cursorId = ref<string | null>(null);

  function cursor(): string | null {
    const order = options.order();
    if (cursorId.value !== null && order.includes(cursorId.value)) return cursorId.value;
    const initial = options.initialCursor?.() ?? null;
    return initial !== null && order.includes(initial) ? initial : null;
  }

  function isSelected(id: string): boolean {
    return options.get().ids.has(id);
  }

  function isCursor(id: string): boolean {
    return cursor() === id;
  }

  function selectOnly(id: string): void {
    options.set({ ids: new Set([id]), anchorId: id });
    cursorId.value = id;
    options.onActivate?.(id);
  }

  function toggle(id: string): void {
    const current = options.get();
    const ids = new Set(current.ids);
    const adding = !ids.has(id);
    if (adding) ids.add(id);
    else ids.delete(id);
    options.set({ ids, anchorId: id });
    cursorId.value = id;
    if (adding) options.onActivate?.(id);
  }

  /** From the anchor (or the cursor) to `id`; `additive` keeps what was already selected. */
  function selectRange(id: string, additive: boolean): void {
    const order = options.order();
    const current = options.get();
    const anchor = current.anchorId !== null && order.includes(current.anchorId) ? current.anchorId : cursor();
    const from = anchor !== null ? order.indexOf(anchor) : -1;
    const to = order.indexOf(id);
    if (from < 0 || to < 0) {
      selectOnly(id);
      return;
    }
    const ids = additive ? new Set(current.ids) : new Set<string>();
    for (let i = Math.min(from, to); i <= Math.max(from, to); i += 1) {
      const each = order[i];
      if (each !== undefined) ids.add(each);
    }
    options.set({ ids, anchorId: anchor });
    cursorId.value = id;
    options.onActivate?.(id);
  }

  function selectAll(): void {
    const order = options.order();
    if (order.length === 0) return;
    const current = options.get();
    options.set({ ids: new Set(order), anchorId: current.anchorId ?? order[0] ?? null });
  }

  function onClick(id: string, event: MouseEvent): void {
    const ctrl = event.ctrlKey || event.metaKey;
    if (event.shiftKey) selectRange(id, ctrl);
    else if (ctrl) toggle(id);
    else selectOnly(id);
  }

  /** Stops Shift-click from selecting the text between the previous click and this one. */
  function onMousedown(event: MouseEvent): void {
    if (event.shiftKey) event.preventDefault();
  }

  /** Handles the list keys; returns true when the event was one of them. */
  function onKeydown(event: KeyboardEvent): boolean {
    const order = options.order();
    if (order.length === 0) return false;
    const ctrl = event.ctrlKey || event.metaKey;
    if (ctrl && !event.shiftKey && !event.altKey && (event.key === "a" || event.key === "A")) {
      event.preventDefault();
      selectAll();
      return true;
    }
    const at = cursor();
    if (ctrl && event.key === " " && at !== null) {
      event.preventDefault();
      toggle(at);
      return true;
    }
    const index = at !== null ? order.indexOf(at) : -1;
    let next: number;
    switch (event.key) {
      case "ArrowDown":
        next = index < 0 ? 0 : Math.min(order.length - 1, index + 1);
        break;
      case "ArrowUp":
        next = index < 0 ? order.length - 1 : Math.max(0, index - 1);
        break;
      case "Home":
        next = 0;
        break;
      case "End":
        next = order.length - 1;
        break;
      default:
        return false;
    }
    event.preventDefault();
    const id = order[next];
    if (id === undefined) return true;
    if (event.shiftKey) selectRange(id, ctrl);
    else if (ctrl) cursorId.value = id;
    else selectOnly(id);
    options.reveal?.(id);
    return true;
  }

  /** Drops ids that are no longer in the list, for example after a status refresh. */
  function prune(): void {
    const present = new Set(options.order());
    const current = options.get();
    if ([...current.ids].every((id) => present.has(id))) return;
    options.set({
      ids: new Set([...current.ids].filter((id) => present.has(id))),
      anchorId: current.anchorId !== null && present.has(current.anchorId) ? current.anchorId : null,
    });
  }

  return { cursorId, cursor, isSelected, isCursor, selectOnly, toggle, selectRange, selectAll, onClick, onMousedown, onKeydown, prune };
}
