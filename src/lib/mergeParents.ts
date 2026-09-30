import type { Remote } from "@/lib/bindings";

/** What the repository has, for naming a merge commit's parents. */
export interface RepoNames {
  /** Short names of the local and remote branches. */
  branches: readonly string[];
  remotes: readonly Remote[];
}

/**
 * Branch names for a merge commit's parents, in parent order, taken from the merge message where
 * it names them. Recognised forms:
 * - Git: `Merge branch 'a' into b`, `Merge branches 'a' and 'b'`, `Merge remote-tracking branch
 *   'origin/a'`, `Merge tag 'v1'`, optionally with ` of <url>`, which `git pull` writes; a URL
 *   that belongs to one of `remotes` turns `a` into `origin/a`. Git leaves out ` into <branch>`
 *   only when merging into main or master, which `defaultTarget` fills in.
 * - GitLab: `Merge branch 'a' into 'b'`.
 * - GitHub: `Merge pull request #12 from owner/a` (the target is not named).
 * - Bitbucket: `Merged in a (pull request #3)` (the target is not named) and `Merged a into b`;
 *   Bitbucket Server: `Merge pull request #3 in PROJ/repo from a to b`.
 * - Gitea and Forgejo: `Merge pull request 'Title' (#12) from a into b`.
 * - Gerrit: `Merge "Title" into b` and `Merge changes I12ab,I34cd into b` (the source is not named).
 * Azure DevOps (`Merged PR 123: Title`) names neither branch.
 */
export function namesFromMergeMessage(subject: string, parentCount: number, remotes: readonly Remote[] = []): MessageNames {
  const names: (string | null)[] = Array.from({ length: parentCount }, () => null);
  const text = subject.trim();
  const found = (merged: string | null, into: string | null): MessageNames => {
    if (parentCount > 1 && merged) names[1] = merged;
    if (into) names[0] = into;
    return { names, targetOmitted: false };
  };

  const git = /^Merge (?:remote-tracking )?(?:branch|branches|tag|commit) (.+?)(?: of (\S+))?(?: into '?([^']+?)'?)?$/.exec(text);
  if (git) {
    const remote = git[2] ? remoteForUrl(git[2], remotes) : null;
    const merged = [...git[1].matchAll(/'([^']+)'/g)].map((m) => (remote ? `${remote}/${m[1]}` : m[1]));
    merged.forEach((name, i) => {
      if (i + 1 < parentCount) names[i + 1] = name;
    });
    if (git[3]) names[0] = git[3];
    return { names, targetOmitted: !git[3] };
  }
  const gitea = /^Merge pull request '.*' \(#\d+\) from (\S+) into (\S+)$/.exec(text);
  if (gitea) return found(gitea[1], gitea[2]);
  const bitbucketServer = /^Merge pull request #\d+ in \S+ from (\S+) to (\S+)/.exec(text);
  if (bitbucketServer) return found(bitbucketServer[1], bitbucketServer[2]);
  const bitbucket = /^Merged in (\S+) \(pull request #\d+\)/.exec(text) ?? /^Merged (\S+) into (\S+)$/.exec(text);
  if (bitbucket) return found(bitbucket[1], bitbucket[2] ?? null);
  const github = /^Merge pull request #\d+ from (\S+)/.exec(text);
  if (github) {
    // `owner/branch`: the branch is everything after the owner.
    const slash = github[1].indexOf("/");
    return found(slash >= 0 ? github[1].slice(slash + 1) : github[1], null);
  }
  const gerrit = /^Merge (?:".*"|changes \S+) into (\S+)$/.exec(text);
  if (gerrit) return found(null, gerrit[1]);
  return { names, targetOmitted: false };
}

/** The remote whose fetch or push URL is `url`, compared without scheme, user and `.git`. */
function remoteForUrl(url: string, remotes: readonly Remote[]): string | null {
  const bare = (u: string) =>
    u
      .trim()
      .toLowerCase()
      .replace(/^[a-z+]+:\/\//, "")
      .replace(/^[^@/]+@/, "")
      .replace(/^([^/:]+):(?!\d)/, "$1/")
      .replace(/\.git\/?$|\/$/, "");
  const want = bare(url);
  return remotes.find((r) => [r.url, r.pushUrl].some((u) => u !== null && bare(u) === want))?.name ?? null;
}

export interface MessageNames {
  names: (string | null)[];
  /** A Git merge message with no ` into <branch>`: the merge went into main or master. */
  targetOmitted: boolean;
}

/**
 * The branch a Git merge message with no ` into <branch>` was merged into. Git leaves the target
 * out only for main and master (the default of `merge.suppressDest` since Git 2.28; master only
 * before that), so it is whichever of those the repository has: the one the graph puts the merge
 * on, else master, else main.
 */
export function defaultTarget(graphBranch: string | null, branchNames: readonly string[]): string | null {
  const bare = graphBranch?.replace(/^[^/]+\/(?=(?:main|master)$)/, "") ?? null;
  if (bare === "main" || bare === "master") return bare;
  for (const name of ["master", "main"]) {
    if (branchNames.some((b) => b === name || b.endsWith(`/${name}`))) return name;
  }
  return null;
}

/**
 * The branch name of each parent of a merge commit: the one the merge message names, else the
 * branch from the graph (`graphBranches`, from `getMergeInfo`: the merge commit's own branch
 * first, since that is the branch it was merged into, then each further parent's), else null.
 */
export function mergeParentNames(subject: string, graphBranches: readonly (string | null)[], repo: RepoNames): (string | null)[] {
  const { names, targetOmitted } = namesFromMergeMessage(subject, graphBranches.length, repo.remotes);
  if (targetOmitted && names[0] === null) names[0] = defaultTarget(graphBranches[0] ?? null, repo.branches);
  return graphBranches.map((branch, i) => names[i] ?? branch);
}

/**
 * A label for each parent of a merge commit: its branch name, else `parent N`. Two parents that
 * come out with the same name, as in `Merge branch 'dev' of <url> into dev`, get their parent
 * number as well.
 */
export function mergeParentLabels(subject: string, graphBranches: readonly (string | null)[], repo: RepoNames): string[] {
  const labels = mergeParentNames(subject, graphBranches, repo).map((name, i) => name ?? `parent ${i + 1}`);
  return labels.map((label, i) => (labels.indexOf(label) !== labels.lastIndexOf(label) ? `${label} (parent ${i + 1})` : label));
}

/**
 * One line for a two-parent merge: `Merged {second parent's branch} into {first parent's branch}`,
 * since the first parent is the branch the merge was made on. With only the merged branch known,
 * `Merged {branch}`; otherwise `Merge commit`.
 */
export function mergeSummary(names: readonly (string | null)[]): string {
  const [into, merged] = names;
  if (merged && into) return `Merged ${merged} into ${into}`;
  if (merged) return `Merged ${merged}`;
  return "Merge commit";
}
