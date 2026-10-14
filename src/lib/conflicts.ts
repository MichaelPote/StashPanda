/**
 * Names for a merge, rebase, cherry-pick or revert in progress and for the two sides of its
 * conflicts, as the user thinks of them: by branch or commit. Git's own words "ours" and "theirs"
 * never reach the screen, because a rebase swaps them: there, "ours" is the branch being rebased
 * onto and "theirs" is the user's own commit being replayed.
 */
import type { ConflictVersion, InProgress, InProgressKind, RefsSnapshot, StatusSnapshot } from "@/lib/bindings";
import { shortSha } from "@/lib/refNames";

/** The command each operation is, for sentences ("Aborting the cherry-pick"). */
export const OPERATION_VERBS: Record<InProgressKind, string> = {
  merge: "merge",
  rebase: "rebase",
  cherryPick: "cherry-pick",
  revert: "revert",
};

/**
 * How messages name a commit: a local branch that points at it, else a remote branch, else a tag,
 * else its short sha. `skip` leaves out one branch (the one being rebased).
 */
export function nameOfCommit(oid: string, refs: RefsSnapshot | null, skip: string | null = null): string {
  const at = (refs?.refs ?? []).filter((r) => r.target === oid && r.fullName !== skip);
  const pick = at.find((r) => r.kind.type === "localBranch") ?? at.find((r) => r.kind.type === "remoteBranch") ?? at.find((r) => r.kind.type === "tag");
  return pick?.short ?? shortSha(oid);
}

export interface OperationNames {
  /** What is happening: "Merging feature into main", "Rebasing main onto origin/main, step 2 of 5". */
  title: string;
  /** Git's stage 2 named for the user: the branch merged into, or for a rebase the branch rebased onto. */
  ours: string;
  /** Git's stage 3 named for the user: what comes in, or for a rebase the commit being replayed. */
  theirs: string;
}

/**
 * The names for the operation in `status`. Without one (conflicts left by applying a stash),
 * the checked-out branch and "the other side".
 */
export function operationNames(status: StatusSnapshot | null, refs: RefsSnapshot | null): OperationNames {
  const op = status?.inProgress ?? null;
  const head = status?.branch ?? "HEAD";
  if (!op) return { title: "", ours: head, theirs: "the other side" };
  return namesFor(op, head, refs);
}

function namesFor(op: InProgress, head: string, refs: RefsSnapshot | null): OperationNames {
  const commit = op.commit === null ? null : shortSha(op.commit);
  switch (op.kind) {
    case "merge": {
      const from = op.commit === null ? "another branch" : nameOfCommit(op.commit, refs);
      return { title: `Merging ${from} into ${head}`, ours: head, theirs: from };
    }
    case "rebase": {
      const branch = op.branch?.replace(/^refs\/heads\//, "") ?? "a detached HEAD";
      const onto = op.onto === null ? "another commit" : nameOfCommit(op.onto, refs, op.branch);
      const step = op.step !== null && op.total !== null ? `, step ${op.step} of ${op.total}` : "";
      const replayed = commit === null ? `${branch}'s commit` : op.branch === null ? commit : `${commit} from ${branch}`;
      return { title: `Rebasing ${branch} onto ${onto}${step}`, ours: onto, theirs: replayed };
    }
    case "cherryPick": {
      const picked = commit ?? "a commit";
      return { title: `Cherry-picking ${picked} onto ${head}`, ours: head, theirs: picked };
    }
    case "revert": {
      const reverted = commit ?? "a commit";
      // Git's stage 3 in a revert is the file as it was before the commit being undone.
      return { title: `Reverting ${reverted} on ${head}`, ours: head, theirs: `before ${reverted}` };
    }
  }
}

/** Which versions of a conflicted file exist, from its two status letters (`UU`, `DU`, `AA`…). */
export interface ConflictStages {
  base: boolean;
  ours: boolean;
  theirs: boolean;
}

/**
 * `A` added and `U` changed both leave that side's version; `D` deleted it. Only a file both sides
 * added (`AA`, `AU`, `UA`) has no base.
 */
export function conflictStages(letters: string): ConflictStages {
  const [x = "U", y = "U"] = [...letters];
  return { base: x !== "A" && y !== "A", ours: x === "A" || x === "U", theirs: y === "A" || y === "U" };
}

/** What each side did to a conflicted file, in one sentence. */
export function conflictSummary(letters: string, names: OperationNames): string {
  const { ours, theirs } = names;
  switch (letters) {
    case "UU":
      return `Both ${ours} and ${theirs} changed this file.`;
    case "AA":
      return `Both ${ours} and ${theirs} added this file, with different contents.`;
    case "DD":
      return `Both sides deleted this file.`;
    case "AU":
      return `${ours} added this file.`;
    case "UA":
      return `${theirs} added this file.`;
    case "DU":
      return `${ours} deleted this file, and ${theirs} changed it.`;
    case "UD":
      return `${ours} changed this file, and ${theirs} deleted it.`;
    default:
      return "This file has a conflict.";
  }
}

/** One way of looking at a conflicted file: two of its versions compared. */
export interface ConflictComparison {
  from: ConflictVersion;
  to: ConflictVersion;
}

export const DEFAULT_COMPARISON: ConflictComparison = { from: "ours", to: "theirs" };

/** The comparisons the diff panel offers for a conflicted file, with their labels and tooltips. */
export function conflictComparisons(names: OperationNames): (ConflictComparison & { label: string; title: string })[] {
  const { ours, theirs } = names;
  return [
    { from: "base", to: "ours", label: `Base to ${ours}`, title: `What ${ours} changed, from the version both sides started from` },
    { from: "base", to: "theirs", label: `Base to ${theirs}`, title: `What ${theirs} changed, from the version both sides started from` },
    { from: "ours", to: "theirs", label: `${ours} to ${theirs}`, title: `How ${theirs}'s version differs from ${ours}'s` },
    {
      from: "ours",
      to: "workingTree",
      label: `${ours} to the working tree`,
      title: `The file on disk, with Git's conflict markers or your edits, against ${ours}'s version`,
    },
  ];
}

/** True when `text` still has a line Git writes to mark a conflict (`<<<<<<< ` or `>>>>>>> `). */
export function hasConflictMarkers(text: string): boolean {
  return /^(<{7}|>{7})( |$)/m.test(text);
}
