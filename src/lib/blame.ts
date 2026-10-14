/**
 * Layout for the Blame dialog (`components/dialogs/BlameDialog.vue`): the blocks of lines drawn
 * together, and how recent each commit is, for shading the blocks by age.
 */
import type { FileBlame } from "@/lib/bindings";
import { shortOid } from "@/lib/format";

/** Neighbouring lines that one commit last changed, drawn as one block with its details on the first line. */
export interface BlameBlock {
  /** Index of the block's first line. */
  start: number;
  count: number;
  /** Index into `FileBlame.commits`. */
  commit: number;
  /** Index into `FileBlame.groups` of the block's first line. */
  group: number;
}

export interface BlameLayout {
  blocks: BlameBlock[];
  /** The block each line belongs to, by line index. */
  blockOf: Uint32Array;
  /** The group each line belongs to, by line index: its origin says which version of the file the line came from. */
  groupOf: Uint32Array;
  /**
   * How recent each commit is among the file's commits, by commit index: 0 for the oldest, 1 for
   * the newest, spaced by rank rather than by date so that one very old commit does not squeeze
   * all the others together. Lines not committed yet count as the newest.
   */
  age: number[];
}

/**
 * Joins Git's groups into blocks. Git starts a new group where a commit's lines were not next to
 * each other in that commit, even when they are next to each other now, so neighbouring groups
 * from the same commit become one block.
 */
export function layoutBlame(blame: FileBlame): BlameLayout {
  const total = blame.lines.length;
  const blockOf = new Uint32Array(total);
  const groupOf = new Uint32Array(total);
  const blocks: BlameBlock[] = [];
  const groups = blame.groups
    .map((g, i) => ({ g, i }))
    .sort((a, b) => a.g.start - b.g.start);
  for (const { g, i } of groups) {
    const commit = blame.origins[g.origin]?.commit ?? 0;
    const last = blocks[blocks.length - 1];
    if (last && last.commit === commit && last.start + last.count === g.start) {
      last.count += g.count;
    } else {
      blocks.push({ start: g.start, count: g.count, commit, group: i });
    }
    const block = blocks.length - 1;
    for (let line = g.start; line < Math.min(total, g.start + g.count); line += 1) {
      blockOf[line] = block;
      groupOf[line] = i;
    }
  }

  const times = [...new Set(blame.commits.filter((c) => !c.uncommitted).map((c) => c.authorTime))].sort((a, b) => a - b);
  const rank = new Map(times.map((t, i) => [t, i]));
  const age = blame.commits.map((c) => {
    if (c.uncommitted || times.length < 2) return 1;
    return (rank.get(c.authorTime) ?? 0) / (times.length - 1);
  });
  return { blocks, blockOf, groupOf, age };
}

/** How the dialogs name a revision: a full commit id is cut to its short form (`1a2b3c4`, `1a2b3c4^`); anything else (`HEAD`) is shown as given. */
export function revisionLabel(rev: string): string {
  return rev.replace(/^(?:[0-9a-f]{64}|[0-9a-f]{40})(?![0-9a-f])/, (oid) => shortOid(oid));
}
