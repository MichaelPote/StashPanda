import type { StashEntry } from "@/lib/bindings";
import { formatAbsoluteTime, shortOid } from "@/lib/format";

/**
 * Splits a stash's reflog message into the text to show and the branch it was made on. Git writes
 * `On main: <message>` when a message was given, and `WIP on main: <short sha> <subject>` when not.
 */
export function stashParts(entry: StashEntry): { text: string; branch: string | null } {
  const match = /^(WIP on|On) (.+?): (.*)$/s.exec(entry.message);
  if (!match) return { text: entry.message, branch: null };
  const [, kind = "", branch = "", rest = ""] = match;
  const text = kind === "WIP on" ? `WIP: ${rest.replace(/^[0-9a-f]{4,40} /, "")}` : rest;
  return { text, branch: branch === "(no branch)" ? null : branch };
}

/** Tooltip for a stash: its selector and commit, where and when it was made, and Git's message. */
export function stashTitle(entry: StashEntry): string {
  const branch = stashParts(entry).branch;
  return [
    `${entry.selector}  ${shortOid(entry.oid)}`,
    branch ? `Made on ${branch}, ${formatAbsoluteTime(entry.time)}` : formatAbsoluteTime(entry.time),
    entry.message,
  ].join("\n");
}
