/**
 * Marking the commit search's matches in the commit list. The backend's `graph_search` decides
 * which rows match (the subject or the author's name or email address, ignoring case, or the
 * start of the commit ID); these mark where the search text appears in what a row shows, so the
 * row can draw a pill around it.
 */

/** A piece of a row's text, and whether it is part of a match. */
export interface Segment {
  text: string;
  match: boolean;
}

/**
 * `text` cut into the parts that match `needle` and the parts between, ignoring case. An empty
 * needle, or text whose lowercase form has a different length (so positions in it would not
 * line up with `text`), gives the whole text unmarked.
 */
export function matchSegments(text: string, needle: string): Segment[] {
  const n = needle.trim().toLowerCase();
  const lower = text.toLowerCase();
  if (n === "" || lower.length !== text.length) return [{ text, match: false }];
  const out: Segment[] = [];
  let at = 0;
  for (let hit = lower.indexOf(n); hit !== -1; hit = lower.indexOf(n, at)) {
    if (hit > at) out.push({ text: text.slice(at, hit), match: false });
    out.push({ text: text.slice(hit, hit + n.length), match: true });
    at = hit + n.length;
  }
  if (at < text.length) out.push({ text: text.slice(at), match: false });
  return out.length > 0 ? out : [{ text, match: false }];
}

/**
 * The shown commit ID (`shown`, the start of `oid`) cut where a commit-ID search matches it: the
 * search matches the start of the ID, so a needle of hex digits that `oid` starts with marks
 * that many characters of it, at most all of `shown`.
 */
export function oidSegments(shown: string, oid: string, needle: string): Segment[] {
  const n = needle.trim().toLowerCase();
  if (n === "" || !/^[0-9a-f]+$/.test(n) || !oid.toLowerCase().startsWith(n)) return [{ text: shown, match: false }];
  const length = Math.min(n.length, shown.length);
  return length === shown.length
    ? [{ text: shown, match: true }]
    : [
        { text: shown.slice(0, length), match: true },
        { text: shown.slice(length), match: false },
      ];
}
