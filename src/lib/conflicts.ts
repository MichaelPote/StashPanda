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
 * the checked-out branch and the stash when it is known, else "the other side".
 */
/**
 * The two sides' names cut short for the Take buttons: each word with a slash keeps only what
 * comes after its last slash (`feature/planting-polygons` is `planting-polygons`,
 * `origin/dependabot/npm_and_yarn/vite-8.3.4` is `vite-8.3.4`). When that makes the two the same
 * (`feature/x` and `origin/feature/x`), each such word keeps what comes before its first slash
 * instead, and when that does too, the full names are used.
 */
export function shortSideNames(names: Pick<OperationNames, "ours" | "theirs">): { ours: string; theirs: string } {
  const cut = (name: string, part: (word: string) => string) => name.split(" ").map((w) => (w.includes("/") ? part(w) : w)).join(" ");
  const last = (w: string) => w.split("/").pop() || w;
  const first = (w: string) => w.slice(0, w.indexOf("/")) || w;
  for (const part of [last, first]) {
    const short = { ours: cut(names.ours, part), theirs: cut(names.theirs, part) };
    if (short.ours !== short.theirs) return short;
  }
  return { ours: names.ours, theirs: names.theirs };
}

/**
 * A Take button's label, `Take planting-polygons (Left)`: Left when `side` is the comparison's
 * left pane (its `from`), Right when it is the right one, and neither when the comparison in the
 * diff panel does not show that side.
 */
export function takeLabel(side: "ours" | "theirs", name: string, comparison: ConflictComparison | null): string {
  const pane = comparison?.from === side ? " (Left)" : comparison?.to === side ? " (Right)" : "";
  return `Take ${name}${pane}`;
}

export function operationNames(status: StatusSnapshot | null, refs: RefsSnapshot | null, stash: string | null = null): OperationNames {
  const op = status?.inProgress ?? null;
  const head = status?.branch ?? "HEAD";
  // Git leaves no marker in the git directory when applying a stash conflicts; `stash` is the
  // one the app applied (RepoSession.stashConflict), which Git's "Stashed changes" side is.
  if (!op) return stash ? { title: `Applying ${stash} to ${head}`, ours: head, theirs: stash } : { title: "", ours: head, theirs: "the other side" };
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
 * The stages Git keeps for each pair of letters, as `git status` names them. The letters are not
 * one per side: `AU` ("added by us") has only our version and `UA` ("added by them") only theirs,
 * which a rename on one side or a file added in a folder the other side renamed leaves.
 */
const STAGES: Record<string, ConflictStages> = {
  UU: { base: true, ours: true, theirs: true },
  AA: { base: false, ours: true, theirs: true },
  UD: { base: true, ours: true, theirs: false },
  DU: { base: true, ours: false, theirs: true },
  AU: { base: false, ours: true, theirs: false },
  UA: { base: false, ours: false, theirs: true },
  DD: { base: true, ours: false, theirs: false },
};

/** Which versions of a conflicted file exist, from its two status letters; all three for letters Git does not use. */
export function conflictStages(letters: string): ConflictStages {
  return STAGES[letters] ?? { base: true, ours: true, theirs: true };
}

/**
 * Why one version of a conflicted file is missing, for a note beside a comparison or the action
 * that takes it. Null when the file has that version.
 */
export function missingVersionNote(letters: string, version: "base" | "ours" | "theirs", names: OperationNames): string | null {
  if (conflictStages(letters)[version]) return null;
  if (version === "base") {
    const added = letters === "AU" ? `${names.ours} added this file` : letters === "UA" ? `${names.theirs} added this file` : "Both sides added this file";
    return `${added}, so there is no base version: the other side shows as wholly added.`;
  }
  const name = version === "ours" ? names.ours : names.theirs;
  const letter = version === "ours" ? letters[0] : letters[1];
  return letter === "D" ? `${name} deleted this file, so that side is empty.` : `${name} does not have this file, so that side is empty.`;
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

/**
 * How every conflict block of a file can be resolved at once: with one side's lines, or both
 * sides' lines one after the other. The lines Git merged cleanly around the blocks stay as they are.
 */
export type TakeAll = "ours" | "theirs" | "oursThenTheirs" | "theirsThenOurs";

/** The four ways to resolve every block, labelled with the sides' names. */
export function takeAllChoices(names: OperationNames): { value: TakeAll; label: string }[] {
  const { ours, theirs } = names;
  return [
    { value: "ours", label: `${ours}'s lines` },
    { value: "theirs", label: `${theirs}'s lines` },
    { value: "oursThenTheirs", label: `${ours}'s lines, then ${theirs}'s` },
    { value: "theirsThenOurs", label: `${theirs}'s lines, then ${ours}'s` },
  ];
}

/** A block's text when resolved by `how`, from its two sides' lines. */
export function takeAllText(block: { ours: string; theirs: string }, how: TakeAll): string {
  switch (how) {
    case "ours":
      return block.ours;
    case "theirs":
      return block.theirs;
    case "oursThenTheirs":
      return block.ours + block.theirs;
    case "theirsThenOurs":
      return block.theirs + block.ours;
  }
}
