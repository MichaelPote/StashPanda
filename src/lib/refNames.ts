/**
 * Client-side check of a branch, tag or remote name before it reaches `git branch`, `git tag` or
 * `git remote`; a remote's name must work as part of a ref name too (`refs/remotes/<name>/…`).
 * Returns a plain-English problem, or null when the name looks acceptable. Git itself is the
 * final judge (see `git check-ref-format`); this only catches the common mistakes early.
 */
export function validateRefName(name: string, what: "branch" | "tag" | "remote"): string | null {
  if (name.length === 0) return `Enter a ${what} name.`;
  if (/\s/.test(name)) return `A ${what} name cannot contain spaces.`;
  if (name.startsWith("-")) return `A ${what} name cannot start with "-".`;
  if (name.startsWith("/") || name.endsWith("/")) return `A ${what} name cannot start or end with "/".`;
  if (name.endsWith(".")) return `A ${what} name cannot end with ".".`;
  if (name.endsWith(".lock")) return `A ${what} name cannot end with ".lock".`;
  for (const bad of ["..", "~", "^", ":", "?", "*", "[", "\\", "@{", "//"]) {
    if (name.includes(bad)) return `A ${what} name cannot contain "${bad}".`;
  }
  if (name === "@") return `"@" is not a valid ${what} name.`;
  return null;
}

/** The first seven characters of a commit id, as Git shows it. */
export function shortSha(oid: string): string {
  return oid.slice(0, 7);
}
