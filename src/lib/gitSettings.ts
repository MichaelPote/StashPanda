import type { ConfigEntry, ConfigScope } from "@/lib/bindings";

/**
 * The common Git settings the Repository Settings dialog explains and edits, grouped as it shows
 * them. Settings StashPanda has no use for (`core.editor`, the `color.*` family) are left out, so
 * nothing listed here does nothing. Everything else in a config file shows under "Other settings".
 */

export type SettingKind =
  | { type: "bool" }
  | { type: "choice"; choices: { value: string; label: string }[] }
  | { type: "text"; placeholder?: string };

export interface GitSetting {
  /** The key as Git's documentation writes it; compared with listed keys ignoring case. */
  key: string;
  group: string;
  kind: SettingKind;
  /** What Git does when the key is set nowhere, in words. */
  gitDefault: string;
  description: string;
  /** Only meaningful in the global file (it applies to repositories yet to be made). */
  globalOnly?: boolean;
}

const MERGE_FF = [
  { value: "true", label: "Fast-forward when possible" },
  { value: "false", label: "Always create a merge commit" },
  { value: "only", label: "Fast-forward only, refuse otherwise" },
];

export const GIT_SETTINGS: GitSetting[] = [
  {
    key: "user.name",
    group: "Identity",
    kind: { type: "text", placeholder: "Your Name" },
    gitDefault: "not set; Git refuses to commit without it",
    description: "The name recorded as the author and committer of your commits.",
  },
  {
    key: "user.email",
    group: "Identity",
    kind: { type: "text", placeholder: "you@example.com" },
    gitDefault: "not set; Git refuses to commit without it",
    description: "The email address recorded with your commits. Hosting sites use it to link commits to your account.",
  },
  {
    key: "push.followTags",
    group: "Pushing",
    kind: { type: "bool" },
    gitDefault: "off",
    description:
      "With every push of a branch, also push the annotated tags that point at the commits being pushed. Lightweight tags are never pushed this way. StashPanda's Push and Sync follow this setting.",
  },
  {
    key: "push.default",
    group: "Pushing",
    kind: {
      type: "choice",
      choices: [
        { value: "simple", label: "Simple: to the branch it tracks, if the names match" },
        { value: "current", label: "Current: to a branch of the same name" },
        { value: "upstream", label: "Upstream: to the branch it tracks, whatever its name" },
        { value: "matching", label: "Matching: every branch the remote has a branch of the same name for" },
        { value: "nothing", label: "Nothing: only where a push names its destination" },
      ],
    },
    gitDefault: "simple",
    description: "Where a push sends a branch when no destination is named. StashPanda's Push follows it, and asks when it would refuse.",
  },
  {
    key: "push.autoSetupRemote",
    group: "Pushing",
    kind: { type: "bool" },
    gitDefault: "off",
    description: "When a branch that tracks nothing is pushed, make it track the branch it was pushed to, as if --set-upstream were given.",
  },
  {
    key: "pull.rebase",
    group: "Pulling",
    kind: {
      type: "choice",
      choices: [
        { value: "false", label: "Merge the remote's commits in" },
        { value: "true", label: "Rebase the branch's own commits on top" },
        { value: "merges", label: "Rebase, keeping the branch's own merge commits" },
      ],
    },
    gitDefault: "merge",
    description:
      "How a pull brings in the remote's commits when the branch has commits of its own. StashPanda's Pull dialog starts with this, unless you chose another way for the repository there.",
  },
  {
    key: "pull.ff",
    group: "Pulling",
    kind: { type: "choice", choices: MERGE_FF },
    gitDefault: "fast-forward when possible",
    description: "Whether a pull that merges may simply move the branch forward when it has no commits of its own.",
  },
  {
    key: "rebase.autoStash",
    group: "Pulling",
    kind: { type: "bool" },
    gitDefault: "off",
    description: "Stash uncommitted changes before a rebase starts and put them back when it ends, instead of refusing to start.",
  },
  {
    key: "rebase.updateRefs",
    group: "Pulling",
    kind: { type: "bool" },
    gitDefault: "off",
    description: "When rebasing a branch, also move any other branches that point at the commits being rebased, so stacked branches stay stacked.",
  },
  {
    key: "fetch.prune",
    group: "Fetching",
    kind: { type: "bool" },
    gitDefault: "off",
    description:
      "At every fetch, remove remote-tracking branches whose branch has been deleted on the remote. StashPanda's Fetch always does this; the setting is for git fetch elsewhere.",
  },
  {
    key: "fetch.pruneTags",
    group: "Fetching",
    kind: { type: "bool" },
    gitDefault: "off",
    description:
      "When a fetch prunes, also delete local tags the remote does not have. That includes tags you made here and never pushed.",
  },
  {
    key: "merge.conflictStyle",
    group: "Merging",
    kind: {
      type: "choice",
      choices: [
        { value: "merge", label: "Merge: both sides" },
        { value: "diff3", label: "diff3: both sides and the original text" },
        { value: "zdiff3", label: "zdiff3: like diff3, with lines both sides share moved out" },
      ],
    },
    gitDefault: "merge",
    description: "How conflicts are written into files. StashPanda's merge editor reads all three; diff3 and zdiff3 also show what the text was before either side changed it.",
  },
  {
    key: "merge.ff",
    group: "Merging",
    kind: { type: "choice", choices: MERGE_FF },
    gitDefault: "fast-forward when possible",
    description: "Whether a merge may simply move the branch forward when it has no commits of its own.",
  },
  {
    key: "rerere.enabled",
    group: "Merging",
    kind: { type: "bool" },
    gitDefault: "off",
    description: "Remember how each conflict was resolved, and resolve the same conflict the same way when it comes up again, as it can during a repeated rebase.",
  },
  {
    key: "commit.gpgSign",
    group: "Signing",
    kind: { type: "bool" },
    gitDefault: "off",
    description:
      "Sign every commit. Git needs a signing key, and for GPG an agent that can ask for its passphrase, since StashPanda cannot type it in for Git.",
  },
  {
    key: "tag.gpgSign",
    group: "Signing",
    kind: { type: "bool" },
    gitDefault: "off",
    description: "Sign every annotated tag.",
  },
  {
    key: "gpg.format",
    group: "Signing",
    kind: {
      type: "choice",
      choices: [
        { value: "openpgp", label: "OpenPGP (GPG)" },
        { value: "ssh", label: "SSH key" },
        { value: "x509", label: "X.509 (S/MIME)" },
      ],
    },
    gitDefault: "OpenPGP",
    description: "Which kind of key signs commits and tags.",
  },
  {
    key: "user.signingKey",
    group: "Signing",
    kind: { type: "text", placeholder: "Key ID, or ~/.ssh/id_ed25519.pub" },
    gitDefault: "the key matching your email address (GPG)",
    description: "The key that signs: a GPG key ID, or for SSH signing the path to a public key file.",
  },
  {
    key: "core.autocrlf",
    group: "Line endings and files",
    kind: {
      type: "choice",
      choices: [
        { value: "true", label: "Convert: CRLF in files here, LF in commits" },
        { value: "input", label: "LF in commits, files here left as they are" },
        { value: "false", label: "Never convert" },
      ],
    },
    gitDefault: "never convert (Git for Windows' installer usually sets convert)",
    description: "Whether Git converts line endings between the files here and what is committed.",
  },
  {
    key: "core.eol",
    group: "Line endings and files",
    kind: {
      type: "choice",
      choices: [
        { value: "native", label: "The operating system's own" },
        { value: "lf", label: "LF" },
        { value: "crlf", label: "CRLF" },
      ],
    },
    gitDefault: "the operating system's own",
    description: "The line ending Git writes text files with here, for files .gitattributes marks as text.",
  },
  {
    key: "core.fileMode",
    group: "Line endings and files",
    kind: { type: "bool" },
    gitDefault: "on (Git sets it when the repository is made)",
    description:
      "Whether a change to a file's executable bit counts as a change. Off on filesystems that do not keep the bit, such as Windows folders opened from WSL.",
  },
  {
    key: "init.defaultBranch",
    group: "New repositories",
    kind: { type: "text", placeholder: "main" },
    gitDefault: "master",
    description: "The name of the first branch in repositories made from now on.",
    globalOnly: true,
  },
  {
    key: "credential.helper",
    group: "Credentials",
    kind: { type: "text", placeholder: "manager" },
    gitDefault: "none",
    description:
      "The program Git asks for usernames and passwords for HTTPS remotes, such as manager (Git Credential Manager) or store. StashPanda cannot ask for them itself, so an HTTPS remote needs one.",
  },
];

/** The groups in the order the dialog shows them. */
export const SETTING_GROUPS = [...new Set(GIT_SETTINGS.map((s) => s.group))];

/** Whether `key` (as Git lists it) is one of the catalogue's. Section and name compare without case. */
export function catalogueSetting(key: string): GitSetting | undefined {
  const lower = key.toLowerCase();
  return GIT_SETTINGS.find((s) => s.key.toLowerCase() === lower);
}

/** The order Git reads its files in: a later one wins for a setting with one value. */
const SCOPE_ORDER = ["system", "global", "local", "worktree"];

/** What applies for `key`: the file it is set in (null: nowhere), and its value. A key with no `=` reads as true. */
export interface SettingValue {
  scope: string | null;
  value: string | null;
}

/**
 * The value `key` has in `scope`'s own file (scope set, or scope null when that file does not set it),
 * and the value it would have there without it: from the files Git reads before it, or Git's default
 * (scope null). For the global tab those are the system file; for the local tab, system and global.
 */
export function settingValues(entries: ConfigEntry[], key: string, scope: ConfigScope): { own: SettingValue; inherited: SettingValue } {
  const lower = key.toLowerCase();
  const matching = entries.filter((e) => e.key.toLowerCase() === lower);
  const last = (scopes: string[]): SettingValue => {
    const hit = [...matching].reverse().find((e) => scopes.includes(e.scope));
    return hit ? { scope: hit.scope, value: hit.value ?? "true" } : { scope: null, value: null };
  };
  const position = SCOPE_ORDER.indexOf(scope);
  return { own: last([scope]), inherited: last(SCOPE_ORDER.slice(0, position)) };
}

/** Git's ways of writing a boolean, as true, false, or null when `value` is not one. */
export function parseBool(value: string | null): boolean | null {
  if (value === null) return null;
  const v = value.trim().toLowerCase();
  if (["true", "yes", "on", "1", ""].includes(v)) return true;
  if (["false", "no", "off", "0"].includes(v)) return false;
  return null;
}

/** How a value reads in the dialog: On and Off for booleans, a choice's label, or the text itself. */
export function describeValue(setting: GitSetting | undefined, value: string): string {
  if (setting?.kind.type === "bool") {
    const b = parseBool(value);
    return b === null ? value : b ? "On" : "Off";
  }
  if (setting?.kind.type === "choice") return setting.kind.choices.find((c) => c.value === value.toLowerCase())?.label ?? value;
  return value;
}

/** The entries in `scope`'s own file that the catalogue does not cover, for "Other settings", in file order. */
export function otherEntries(entries: ConfigEntry[], scope: ConfigScope): ConfigEntry[] {
  return entries.filter((e) => e.scope === scope && !catalogueSetting(e.key));
}
