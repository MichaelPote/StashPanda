/**
 * Which tags are pushed: the tags here compared with each remote's, from `git ls-remote --tags`.
 * Git keeps no record of a remote's tags (there is no `refs/remotes/origin/tags/…`), so the only
 * way to know is to ask the remote.
 */

/** One remote's tags, name to the commit each ends at; null when the remote could not be reached. */
export type RemoteTagAnswer = Map<string, string> | null;

/**
 * A tag here: `both` when every remote compared has it on the same commit, `localOnly` when one
 * of them lacks it or has it on another commit, `unknown` while a remote has not answered (or
 * could not be reached, or there are no remotes), which draws the tag as before.
 */
export type TagSyncState = "both" | "localOnly" | "unknown";

export interface LocalTagSync {
  state: TagSyncState;
  /** Remotes compared that do not have the tag. */
  missingOn: string[];
  /** Remotes compared that have the tag on another commit. */
  elsewhereOn: string[];
}

/** A tag a remote has that is not here, or is here on another commit. */
export interface RemoteOnlyTag {
  name: string;
  commit: string;
  /** The remotes that have it on `commit`. */
  remotes: string[];
}

export interface TagSyncView {
  /** Each tag here, by name. */
  local: Map<string, LocalTagSync>;
  /** Remote tags to draw as their own pill, by the commit they are on. */
  remoteOnly: Map<string, RemoteOnlyTag[]>;
  /** Remote tags with no tag of that name here at all, for the Branches panel's list. */
  notHere: RemoteOnlyTag[];
}

/**
 * Compares `localTags` (name to commit) with the `answers` of the `compared` remotes. A remote
 * missing from `answers` has not answered yet.
 */
export function tagSyncView(localTags: Map<string, string>, answers: Map<string, RemoteTagAnswer>, compared: string[]): TagSyncView {
  const local = new Map<string, LocalTagSync>();
  for (const [name, commit] of localTags) {
    const missingOn: string[] = [];
    const elsewhereOn: string[] = [];
    let unanswered = compared.length === 0;
    for (const remote of compared) {
      const answer = answers.get(remote);
      if (!answer) {
        unanswered = true;
        continue;
      }
      const there = answer.get(name);
      if (there === undefined) missingOn.push(remote);
      else if (there !== commit) elsewhereOn.push(remote);
    }
    const state: TagSyncState = missingOn.length > 0 || elsewhereOn.length > 0 ? "localOnly" : unanswered ? "unknown" : "both";
    local.set(name, { state, missingOn, elsewhereOn });
  }

  // One entry per tag name and commit, gathering the remotes that agree on it.
  const found = new Map<string, RemoteOnlyTag>();
  for (const remote of compared) {
    const answer = answers.get(remote);
    if (!answer) continue;
    for (const [name, commit] of answer) {
      if (localTags.get(name) === commit) continue;
      const id = `${name}\n${commit}`;
      const entry = found.get(id);
      if (entry) entry.remotes.push(remote);
      else found.set(id, { name, commit, remotes: [remote] });
    }
  }
  const remoteOnly = new Map<string, RemoteOnlyTag[]>();
  const notHere: RemoteOnlyTag[] = [];
  for (const tag of found.values()) {
    const onCommit = remoteOnly.get(tag.commit);
    if (onCommit) onCommit.push(tag);
    else remoteOnly.set(tag.commit, [tag]);
    if (!localTags.has(tag.name)) notHere.push(tag);
  }
  notHere.sort((a, b) => a.name.localeCompare(b.name) || a.commit.localeCompare(b.commit));
  return { local, remoteOnly, notHere };
}

/** "origin", "origin and backup", "origin, backup and github". */
function listRemotes(remotes: string[]): string {
  return remotes.length <= 1 ? (remotes[0] ?? "") : `${remotes.slice(0, -1).join(", ")} and ${remotes[remotes.length - 1]}`;
}

/** The line a local-only tag's tooltip adds: which remotes lack it, or have it elsewhere. */
export function localOnlyNote(sync: LocalTagSync): string {
  const parts: string[] = [];
  if (sync.missingOn.length > 0) parts.push(`Not on ${listRemotes(sync.missingOn)}.`);
  if (sync.elsewhereOn.length > 0) parts.push(`On another commit on ${listRemotes(sync.elsewhereOn)}.`);
  return parts.join(" ");
}

/** The line a remote-only tag's tooltip shows. */
export function remoteOnlyNote(tag: RemoteOnlyTag, hereElsewhere: boolean): string {
  const where = `On ${listRemotes(tag.remotes)} only`;
  return hereElsewhere ? `${where} on this commit; the tag here is on another commit.` : `${where}; not fetched into this repository.`;
}
