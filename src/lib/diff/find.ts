/**
 * Find in a diff (Ctrl+F in a diff view): where a text occurs in a line, and the line's spans cut
 * so that each match can be drawn in the find colour, the current match more strongly.
 */
import type { Span } from "@/lib/diff/syntax";

/** One match in one line: from `start` to `end` (UTF-16 offsets), and its number among all matches. */
export interface FindRange {
  start: number;
  end: number;
  ordinal: number;
}

/**
 * Where `needle` occurs in `text`, ignoring case, without overlaps. Lower-casing can change a
 * string's length (`İ` becomes two characters), which would shift every offset after it; such a
 * line is searched with case taken into account instead.
 */
export function findInText(text: string, needle: string): [number, number][] {
  if (needle === "") return [];
  let hay = text.toLowerCase();
  let pin = needle.toLowerCase();
  if (hay.length !== text.length || pin.length !== needle.length) {
    hay = text;
    pin = needle;
  }
  const out: [number, number][] = [];
  let from = 0;
  for (;;) {
    const at = hay.indexOf(pin, from);
    if (at < 0) return out;
    out.push([at, at + pin.length]);
    from = at + pin.length;
  }
}

/**
 * Cuts `spans` at the edges of `ranges` and marks the pieces inside a match: `found` is 1 for a
 * match and 2 for the current one (`current` is its ordinal). The spans' colours and changed-word
 * marks are kept, so a match inside a changed word stays inside its highlight.
 */
export function markFound(spans: Span[], ranges: FindRange[], current: number): Span[] {
  if (ranges.length === 0) return spans;
  const out: Span[] = [];
  let offset = 0;
  let r = 0;
  for (const span of spans) {
    const end = offset + span.text.length;
    let pos = offset;
    while (pos < end) {
      while (r < ranges.length && (ranges[r] as FindRange).end <= pos) r += 1;
      const range = ranges[r];
      if (!range || range.start >= end) {
        out.push(pos === offset ? span : { ...span, text: span.text.slice(pos - offset) });
        break;
      }
      if (range.start > pos) {
        out.push({ ...span, text: span.text.slice(pos - offset, range.start - offset) });
        pos = range.start;
      }
      const stop = Math.min(range.end, end);
      out.push({ ...span, text: span.text.slice(pos - offset, stop - offset), found: range.ordinal === current ? 2 : 1 });
      pos = stop;
    }
    offset = end;
  }
  return out;
}

/** The classes for one piece of a line's text: the find colour on a match, outlined on the current one. */
export function glyphClass(span: Span): string {
  if (span.gap) return "diff-gap-del";
  return span.found === 2 ? "diff-glyphs diff-find diff-find-current" : span.found === 1 ? "diff-glyphs diff-find" : "diff-glyphs";
}

/**
 * Scrolls `target` sideways so that `el` (the current match) shows inside `area`, the box its
 * text is clipped to, with some room beside it. `fixed` is how wide a part at the left of `area`
 * stays in place while the text scrolls under it, such as the Blame dialog's commit column.
 */
export function scrollSidewaysTo(el: HTMLElement, target: HTMLElement, area: HTMLElement, fixed: number): void {
  const margin = 32;
  const r = el.getBoundingClientRect();
  const box = area.getBoundingClientRect();
  const left = box.left + fixed;
  const right = box.left + area.clientWidth;
  if (r.left < left + margin) target.scrollLeft -= left + margin - r.left;
  else if (r.right > right - margin) target.scrollLeft += r.right - (right - margin);
}
