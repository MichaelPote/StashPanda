import type { RefEntry } from "@/lib/bindings";

/**
 * `names` with the branches tracked together with them added: for a local branch, the remote
 * branch it tracks; for a remote branch, every local branch that tracks it. Used when
 * `appearance.showHideTrackedTogether` makes hiding or showing one do the same to the others.
 */
export function withTrackedBranches(refs: readonly RefEntry[], names: readonly string[]): string[] {
  const given = new Set(names);
  const out = new Set(names);
  for (const r of refs) {
    if (r.kind.type !== "localBranch" || !r.upstream?.startsWith("refs/remotes/") || r.upstreamGone) continue;
    if (given.has(r.fullName)) out.add(r.upstream);
    if (given.has(r.upstream)) out.add(r.fullName);
  }
  return [...out];
}
