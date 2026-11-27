/**
 * Renaming a branch on a remote: what Git said when the push that does it was refused.
 *
 * The rename is one push that creates the new name and deletes the old one together (`git push
 * --atomic`, see `rename_remote_branch` in the backend), so a refusal of either half leaves the
 * remote as it was. Git marks the half that was refused, and the other one as "atomic push
 * failed".
 */
import type { OpResult } from "@/lib/bindings";

/**
 * Why renaming `branch` to `to` on `remote` was refused, in words that say what to do next, or
 * null for a failure this does not recognise (the caller then shows Git's own message).
 */
export function remoteRenameRefusal(result: OpResult, remote: string, branch: string, to: string): string | null {
  const said = `${result.stdout}\n${result.stderr}`;
  // ` ! [rejected]  <commit> -> <to> (stale info)`: the new name was to be created, and exists.
  if (said.includes(`-> ${to} (stale info)`)) {
    return `${remote} already has a branch ${to}, which this repository has not fetched yet. Nothing was changed. Fetch, then choose another name.`;
  }
  // ` ! [rejected]  (delete) -> <branch> (stale info)`: the old name is not where it was at the last fetch.
  if (said.includes(`-> ${branch} (stale info)`)) {
    return `${branch} on ${remote} has commits that this repository has not fetched, so it was not renamed: the new name would have been made without them. Nothing was changed. Fetch, then rename it again.`;
  }
  if (/refusing to delete the current branch|deletion of the current branch prohibited/i.test(said)) {
    return `${remote} will not rename ${branch} because it is the remote's default branch, which it does not let anyone delete. Nothing was changed. Rename it in the repository's settings on the server instead.`;
  }
  if (/does not support --atomic/i.test(said)) {
    return `${remote} cannot make two changes in one push, which renaming needs so that the branch is never left under both names or neither. Nothing was changed. Push the branch under the new name, then delete the old one.`;
  }
  if (said.includes("[remote rejected]")) {
    return `${remote} refused to rename ${branch}: it may be a protected branch, or your account may not be allowed to delete branches there. Nothing was changed.`;
  }
  return null;
}

/** The local branches (short names) that track `<remote>/<branch>` and so need pointing at its new name. */
export function branchesTracking(refs: readonly { kind: { type: string }; short: string; upstream: string | null; upstreamGone: boolean }[], remote: string, branch: string): string[] {
  const upstream = `refs/remotes/${remote}/${branch}`;
  return refs.filter((r) => r.kind.type === "localBranch" && r.upstream === upstream && !r.upstreamGone).map((r) => r.short);
}

/** `a`, `a and b`, or `a, b and c`. */
export function listNames(names: readonly string[]): string {
  if (names.length <= 1) return names[0] ?? "";
  return `${names.slice(0, -1).join(", ")} and ${names[names.length - 1]}`;
}
