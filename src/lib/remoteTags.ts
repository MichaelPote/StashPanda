import { commands, type RemoteTag } from "@/lib/bindings";
import { errorMessage, unwrap } from "@/lib/ipc";
import { shortSha } from "@/lib/refNames";

/**
 * What a tag dialog knows about one remote's copy of the tag. It starts as `checking`, and the
 * dialog acts as if the remote did not have the tag until `git ls-remote` answers.
 */
export type RemoteTagState =
  | { state: "checking" }
  | { state: "missing" }
  | { state: "found"; tag: RemoteTag }
  | { state: "failed"; message: string };

/** What a tag dialog does on the remotes it ticks. */
export type RemoteTagAction = "remove" | "rename" | "move";

/**
 * Asks each of `remotes` in the background whether it has tag `name`, calling `onResult` as each
 * answers. Returns a function that stops the checks still running (when the dialog closes) and
 * drops their answers.
 */
export function checkRemoteTags(key: string, name: string, remotes: string[], onResult: (remote: string, state: RemoteTagState) => void): () => void {
  let stopped = false;
  const running = new Map<string, string>();
  for (const remote of remotes) {
    const opId = crypto.randomUUID();
    running.set(remote, opId);
    unwrap(commands.queryRemoteTag(key, opId, remote, name))
      .then((lookup): RemoteTagState => {
        if (!lookup.result.ok) return { state: "failed", message: lookupFailure(lookup.result.stderr) };
        return lookup.tag ? { state: "found", tag: lookup.tag } : { state: "missing" };
      })
      .catch((e: unknown): RemoteTagState => ({ state: "failed", message: errorMessage(e) }))
      .then((state) => {
        running.delete(remote);
        if (!stopped) onResult(remote, state);
      });
  }
  return () => {
    stopped = true;
    for (const opId of running.values()) void commands.opCancel(key, opId);
    running.clear();
  };
}

/** The useful line of a failed `ls-remote`: Git's `fatal:` line, else its last line. */
function lookupFailure(stderr: string): string {
  const lines = stderr.split("\n").map((l) => l.trim()).filter((l) => l.length > 0);
  const fatal = lines.find((l) => l.startsWith("fatal:"));
  return (fatal ?? lines[lines.length - 1] ?? "Git gave no reason.").replace(/^fatal:\s*/, "");
}

/** The checkbox for `remote` in the dialog for `action`. */
export function remoteTagLabel(action: RemoteTagAction, remote: string): string {
  if (action === "remove") return `Also remove from ${remote}`;
  if (action === "rename") return `Also rename on ${remote}`;
  return `Also move on ${remote} (git push --force)`;
}

/**
 * False once the remote is known not to have the tag and ticking it would do nothing: there is
 * nothing to remove or rename there. Moving still pushes the tag to it.
 */
export function remoteTagTickable(action: RemoteTagAction, state: RemoteTagState): boolean {
  return action === "move" || state.state !== "missing";
}

/**
 * The line under the checkbox: what the remote has, and what ticking it does. `tag` is the tag's
 * name, `commit` the commit it is on here (for Move Tag, the one it is moving from), and
 * `renamed` the new name for Rename Tag.
 */
export function remoteTagNote(action: RemoteTagAction, remote: string, state: RemoteTagState, tag: string, commit: string, renamed = ""): string {
  const newName = renamed.length > 0 ? renamed : "the new name";
  if (state.state === "checking") return `Checking whether ${remote} has ${tag}…`;
  if (state.state === "failed") return `Could not check ${remote}: ${state.message}`;
  if (state.state === "missing") {
    if (action === "move") return `${remote} does not have ${tag}, so this pushes it there.`;
    return `${remote} does not have ${tag}.`;
  }
  const same = state.tag.commit === commit;
  const where = same ? "on the same commit" : `on another commit, ${shortSha(state.tag.commit)}`;
  if (action === "remove") return same ? `${remote} has ${tag} ${where}.` : `${remote} has ${tag} ${where}, which is removed too.`;
  if (action === "rename") {
    return same
      ? `${remote} has ${tag} ${where}. This pushes ${newName} there, then deletes ${tag}.`
      : `${remote} has ${tag} ${where}. This pushes ${newName} on this repository's commit, then deletes ${remote}'s ${tag}.`;
  }
  return `${remote} has ${tag} ${where}. This replaces it there; anyone who has already fetched ${tag} keeps the old one until they fetch it with --force.`;
}

/**
 * The lease for moving a tag on a remote (`opPushTagForce`'s `expected`): the object the dialog
 * found there, "" when it found none, or null for a plain --force when the check had not answered.
 */
export function moveLease(found: RemoteTag | null | undefined): string | null {
  if (found === undefined) return null;
  return found === null ? "" : found.oid;
}
