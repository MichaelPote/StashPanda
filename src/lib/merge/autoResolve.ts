/**
 * Resolving simple conflicts: the ones where both sides changed the file, but in lines that only
 * touch, which Git reports as a conflict although neither change overlaps the other. Keeping both
 * changes resolves them.
 *
 * Git's conflict markers carry no base text in its default style, so each block is found again
 * by merging the three versions in the index the way Git does (touching changes conflict, and
 * lines both sides share at a conflict's edges are moved out of it), and matched to its block by
 * the two sides' text. The block is then merged again with touching changes allowed. A block that
 * matches nothing, or still conflicts, is left alone, so a difference between this comparison
 * and Git's never resolves a block wrongly.
 */
import { diff } from "@codemirror/merge";
import type { ConflictFile } from "@/lib/bindings";

/** What this needs of one of the backend's conflict blocks (`ConflictBlock`). */
export interface BlockSides {
  ours: string;
  base: string | null;
  theirs: string;
}

/** What this needs of a conflicted file (`ConflictFile`): stages 1, 2 and 3, and its blocks. */
export interface ConflictVersions {
  base: string | null;
  ours: string | null;
  theirs: string | null;
  blocks: readonly BlockSides[];
}

/** A file's lines, each with its "\n", the last one included: block texts end every line with one. */
export function splitLines(text: string): string[] {
  if (text === "") return [];
  const parts = text.split("\n");
  const last = parts.pop() as string;
  const lines = parts.map((l) => `${l}\n`);
  if (last !== "") lines.push(last);
  return lines;
}

/** One change from base to a side: base lines `[b0, b1)` replaced by `lines`. */
interface Hunk {
  side: 0 | 1;
  b0: number;
  b1: number;
  lines: string[];
}

/** Characters standing for lines, skipping the control characters and UTF-16 surrogates. */
const FIRST_CODE = 0x100;
const SURROGATES = [0xd800, 0xdfff] as const;
const MAX_CODE = 0xffff;

/** The changes from `base` to `other`, line by line, or null when there are too many distinct lines to compare. */
function lineHunks(base: string[], other: string[], side: 0 | 1, codes: Map<string, string>): Hunk[] | null {
  const encode = (lines: string[]): string | null => {
    let out = "";
    for (const line of lines) {
      let c = codes.get(line);
      if (c === undefined) {
        let code = FIRST_CODE + codes.size;
        if (code >= SURROGATES[0]) code += SURROGATES[1] - SURROGATES[0] + 1;
        if (code > MAX_CODE) return null;
        c = String.fromCharCode(code);
        codes.set(line, c);
      }
      out += c;
    }
    return out;
  };
  const a = encode(base);
  const b = encode(other);
  if (a === null || b === null) return null;
  return diff(a, b, { scanLimit: 10_000, timeout: 1000 }).map((c) => ({ side, b0: c.fromA, b1: c.toA, lines: other.slice(c.fromB, c.toB) }));
}

/** A run of the merged file: lines both sides agree on, or a conflict with each side's lines. */
export type Segment = { kind: "same"; lines: string[] } | { kind: "conflict"; base: string[]; ours: string[]; theirs: string[] };

function sameLines(a: readonly string[], b: readonly string[]): boolean {
  return a.length === b.length && a.every((l, i) => l === b[i]);
}

/** Base lines `[c0, c1)` with one side's hunks in that range applied. */
function applyHunks(base: string[], c0: number, c1: number, hunks: Hunk[]): string[] {
  const out: string[] = [];
  let at = c0;
  for (const h of hunks) {
    out.push(...base.slice(at, h.b0), ...h.lines);
    at = h.b1;
  }
  out.push(...base.slice(at, c1));
  return out;
}

/**
 * A three-way merge of lines. With `touching` (what Git does), changes from the two sides that
 * touch conflict; without it, only changes that overlap do, or two insertions at the same place.
 * Changes both sides made the same way are taken once. Null when the files are too varied to compare.
 */
export function merge3(base: string[], ours: string[], theirs: string[], touching: boolean): Segment[] | null {
  const codes = new Map<string, string>();
  const o = lineHunks(base, ours, 0, codes);
  const t = lineHunks(base, theirs, 1, codes);
  if (!o || !t) return null;
  // Insertions before a change at the same place.
  const all = [...o, ...t].sort((x, y) => x.b0 - y.b0 || x.b1 - y.b1);
  const out: Segment[] = [];
  const pushSame = (lines: string[]): void => {
    if (lines.length === 0) return;
    const last = out[out.length - 1];
    if (last?.kind === "same") last.lines.push(...lines);
    else out.push({ kind: "same", lines: [...lines] });
  };
  let at = 0;
  let i = 0;
  while (i < all.length) {
    const first = all[i] as Hunk;
    const cluster = [first];
    let c0 = first.b0;
    let c1 = first.b1;
    i += 1;
    while (i < all.length) {
      const h = all[i] as Hunk;
      const joins = touching ? h.b0 <= c1 : h.b0 < c1 || (h.b0 === h.b1 && c0 === c1 && h.b0 === c0);
      if (!joins) break;
      cluster.push(h);
      c0 = Math.min(c0, h.b0);
      c1 = Math.max(c1, h.b1);
      i += 1;
    }
    pushSame(base.slice(at, c0));
    const mine = cluster.filter((h) => h.side === 0);
    const other = cluster.filter((h) => h.side === 1);
    const oursText = applyHunks(base, c0, c1, mine);
    const theirsText = applyHunks(base, c0, c1, other);
    if (mine.length === 0) pushSame(theirsText);
    else if (other.length === 0 || sameLines(oursText, theirsText)) pushSame(oursText);
    else out.push({ kind: "conflict", base: base.slice(c0, c1), ours: oursText, theirs: theirsText });
    at = c1;
  }
  pushSame(base.slice(at));
  return out;
}

/** The lines both sides share at their start and at their end, as counts that never overlap. */
function sharedEdges(a: string[], b: string[]): [number, number] {
  let head = 0;
  while (head < a.length && head < b.length && a[head] === b[head]) head += 1;
  let tail = 0;
  while (tail < a.length - head && tail < b.length - head && a[a.length - 1 - tail] === b[b.length - 1 - tail]) tail += 1;
  return [head, tail];
}

/**
 * Each block's text with its simple conflict resolved, or null for a block that is not simple
 * (or could not be matched to Git's merge). Every entry is null when the file lacks a side.
 */
export function resolveSimpleBlocks(file: ConflictVersions): (string | null)[] {
  const none = file.blocks.map(() => null);
  if (file.ours === null || file.theirs === null || file.blocks.length === 0) return none;
  // A block's every line ends in "\n"; so must the versions' last lines, to compare with them.
  const withNewline = (text: string): string => (text === "" || text.endsWith("\n") ? text : `${text}\n`);
  const base = splitLines(withNewline(file.base ?? ""));
  const strict = merge3(base, splitLines(withNewline(file.ours)), splitLines(withNewline(file.theirs)), true);
  if (!strict) return none;
  // Where each conflict is in the segments.
  const at = strict.flatMap((s, i) => (s.kind === "conflict" ? [i] : []));
  let next = 0;
  return file.blocks.map((block) => {
    for (let k = next; k < at.length; k += 1) {
      // Git joins conflicts that only a few short lines separate into one block, so a block can
      // be a run of them with the lines between.
      for (let m = k; m < Math.min(at.length, k + MAX_JOINED); m += 1) {
        const run = new Run(strict.slice(at[k], (at[m] as number) + 1));
        if (block.base !== null && block.base !== run.base.join("")) continue;
        // Git's default style moves the lines both sides share at the edges out of the block;
        // the `diff3` style keeps them in.
        const [head, tail] = sharedEdges(run.ours, run.theirs);
        const trimmed = (lines: string[]): string => lines.slice(head, lines.length - tail).join("");
        const asIs = block.ours === run.ours.join("") && block.theirs === run.theirs.join("");
        const asTrimmed = block.ours === trimmed(run.ours) && block.theirs === trimmed(run.theirs);
        if (!asIs && !asTrimmed) continue;
        next = m + 1;
        return run.resolve(asIs ? 0 : head, asIs ? 0 : tail);
      }
    }
    return null;
  });
}

/** The most conflicts one of Git's blocks is matched against. */
const MAX_JOINED = 8;

/** Conflicts from Git's way of merging, with the lines between them: one block as Git wrote it. */
class Run {
  readonly base: string[];
  readonly ours: string[];
  readonly theirs: string[];

  constructor(readonly segments: Segment[]) {
    const side = (pick: (c: Extract<Segment, { kind: "conflict" }>) => string[]): string[] =>
      segments.flatMap((s) => (s.kind === "same" ? s.lines : pick(s)));
    this.base = side((c) => c.base);
    this.ours = side((c) => c.ours);
    this.theirs = side((c) => c.theirs);
  }

  /** The run merged again allowing touching changes, without its first `head` and last `tail` lines; null if any of it still conflicts. */
  resolve(head: number, tail: number): string | null {
    const lines: string[] = [];
    for (const s of this.segments) {
      if (s.kind === "same") {
        lines.push(...s.lines);
        continue;
      }
      const merged = merge3(s.base, s.ours, s.theirs, false);
      if (!merged || merged.some((m) => m.kind === "conflict")) return null;
      lines.push(...merged.flatMap((m) => (m.kind === "same" ? m.lines : [])));
    }
    // The shared lines stay outside the block, so the merge must keep them at its edges.
    if (lines.length < head + tail) return null;
    if (!sameLines(lines.slice(0, head), this.ours.slice(0, head))) return null;
    if (tail > 0 && !sameLines(lines.slice(lines.length - tail), this.ours.slice(this.ours.length - tail))) return null;
    return lines.slice(head, lines.length - tail).join("");
  }
}

/** The working tree file with each of its conflict blocks replaced by `texts`, as it is saved. */
export function resolvedFileText(file: ConflictFile, texts: readonly string[]): string {
  const lines = splitLines(file.result);
  // Last block first, so the earlier blocks' line numbers still hold.
  file.blocks
    .map((b, i) => ({ b, text: texts[i] ?? "" }))
    .sort((x, y) => y.b.startLine - x.b.startLine)
    .forEach(({ b, text }) => lines.splice(b.startLine, b.endLine - b.startLine, ...splitLines(text)));
  let out = lines.join("");
  if (file.crlf) out = out.replace(/\n/g, "\r\n");
  return file.bom ? `\u{feff}${out}` : out;
}
