/**
 * The file list items shown in the Files panel, shared with the Commit dialog so both agree on
 * which files a selection names.
 */
import type { ChangeStatus, CommitPreselect, DiffTarget, StatusSnapshot } from "@/lib/bindings";

export type FileState = "added" | "modified" | "deleted" | "renamed" | "conflict" | "untracked" | "neutral";

/** One clickable line in the Files panel. `id` is `<section>:<path>`, unique within one list. */
export interface FileItem {
  id: string;
  path: string;
  oldPath: string | null;
  letter: string;
  state: FileState;
  added: number | null;
  deleted: number | null;
  binary: boolean;
  target: DiffTarget;
}

export interface FileSection {
  key: string;
  title: string;
  items: FileItem[];
}

/** Badge and selected-row band per file state, written out in full so Tailwind finds them. */
export const STATE_CLASSES: Record<FileState, { badge: string; band: string }> = {
  added: { badge: "badge-added", band: "band-added" },
  modified: { badge: "badge-modified", band: "band-modified" },
  deleted: { badge: "badge-deleted", band: "band-deleted" },
  renamed: { badge: "badge-renamed", band: "band-renamed" },
  conflict: { badge: "badge-conflict", band: "band-conflict" },
  untracked: { badge: "badge-untracked", band: "band-untracked" },
  neutral: { badge: "badge-neutral", band: "band-untracked" },
};

export function letterState(letter: string): FileState {
  switch (letter.charAt(0)) {
    case "A":
      return "added";
    case "M":
      return "modified";
    case "D":
      return "deleted";
    case "R":
    case "C":
      return "renamed";
    case "U":
      return "conflict";
    case "?":
      return "untracked";
    default:
      return "neutral";
  }
}

export function statusLetter(status: ChangeStatus): string {
  switch (status) {
    case "added":
      return "A";
    case "modified":
      return "M";
    case "deleted":
      return "D";
    case "renamed":
      return "R";
    case "copied":
      return "C";
    case "typeChanged":
      return "T";
    case "unmerged":
      return "U";
    case "unknown":
      return "?";
  }
}

export function splitPath(path: string): { dir: string; name: string } {
  const idx = path.lastIndexOf("/");
  return idx < 0 ? { dir: "", name: path } : { dir: path.slice(0, idx + 1), name: path.slice(idx + 1) };
}

function worktreeItem(
  path: string,
  oldPath: string | null,
  section: string,
  letter: string,
  target: DiffTarget,
  state: FileState = letterState(letter),
): FileItem {
  return { id: `${section}:${path}`, path, oldPath, letter, state, added: null, deleted: null, binary: false, target };
}

/**
 * The Staged, Unstaged, Untracked and Conflicted sections for a status snapshot. A file with
 * both staged and unstaged changes appears in both of the first two sections.
 */
export function worktreeSections(status: StatusSnapshot | null): FileSection[] {
  const staged: FileItem[] = [];
  const unstaged: FileItem[] = [];
  const untracked: FileItem[] = [];
  const conflicted: FileItem[] = [];
  for (const e of status?.entries ?? []) {
    if (e.ignored) continue;
    if (e.conflicted) {
      conflicted.push(worktreeItem(e.path, e.origPath, "conflicted", `${e.x}${e.y}`.replace(/\./g, ""), { type: "unstaged" }, "conflict"));
      continue;
    }
    if (e.untracked) {
      untracked.push(worktreeItem(e.path, e.origPath, "untracked", "?", { type: "untracked" }));
      continue;
    }
    if (e.staged || e.x !== ".") staged.push(worktreeItem(e.path, e.origPath, "staged", e.x, { type: "staged" }));
    if (e.unstaged || (e.y !== "." && e.y !== "?")) unstaged.push(worktreeItem(e.path, e.origPath, "unstaged", e.y, { type: "unstaged" }));
  }
  return [
    { key: "staged", title: "Staged", items: staged },
    { key: "unstaged", title: "Unstaged", items: unstaged },
    { key: "untracked", title: "Untracked", items: untracked },
    { key: "conflicted", title: "Conflicted", items: conflicted },
  ];
}

/** One file as the Commit dialog lists it: a path appears once even when it is in two sections. */
export interface CommitFile {
  path: string;
  oldPath: string | null;
  letter: string;
  state: FileState;
  /** Has changes in the index. */
  staged: boolean;
  /** Has changes on disk that are not staged, or is untracked. */
  unstaged: boolean;
}

/**
 * Every file the Commit dialog lists, and the paths it starts with ticked: the files selected in
 * the working tree list when there are any, otherwise what `preselect` says. Conflicted files are
 * listed but never ticked for you.
 */
export function commitFiles(
  status: StatusSnapshot | null,
  selectedIds: ReadonlySet<string>,
  preselect: CommitPreselect,
): { files: CommitFile[]; ticked: string[]; fromSelection: boolean } {
  const byPath = new Map<string, CommitFile>();
  const selectedPaths = new Set<string>();
  for (const section of worktreeSections(status)) {
    for (const i of section.items) {
      if (selectedIds.has(i.id)) selectedPaths.add(i.path);
      const staged = section.key === "staged";
      const existing = byPath.get(i.path);
      if (existing) {
        existing.staged ||= staged;
        existing.unstaged ||= !staged;
        continue;
      }
      byPath.set(i.path, { path: i.path, oldPath: i.oldPath, letter: i.letter, state: i.state, staged, unstaged: !staged });
    }
  }
  const files = [...byPath.values()].sort((a, b) => a.path.localeCompare(b.path));
  const fromSelection = selectedPaths.size > 0;
  const ticked = fromSelection
    ? files.filter((f) => selectedPaths.has(f.path)).map((f) => f.path)
    : files.filter((f) => f.state !== "conflict" && (preselect === "all" || f.staged)).map((f) => f.path);
  return { files, ticked, fromSelection };
}

/** How many files the Commit dialog lists when opened: every changed path once, conflicted included. */
export function commitFileCount(status: StatusSnapshot | null): number {
  return commitFiles(status, new Set(), "all").files.length;
}

/**
 * The `paths` argument for `opCommit`. Null when the ticked files are exactly the staged ones,
 * which commits the index as it is, so a partly staged file keeps its unstaged part out of the
 * commit. Otherwise each ticked path, plus the original path of a rename; those are committed as
 * they are on disk.
 */
export function commitPaths(files: CommitFile[], ticked: ReadonlySet<string>): string[] | null {
  const chosen = files.filter((f) => ticked.has(f.path));
  const staged = files.filter((f) => f.staged);
  if (chosen.length > 0 && chosen.length === staged.length && chosen.every((f) => f.staged)) return null;
  const paths: string[] = [];
  for (const f of chosen) {
    paths.push(f.path);
    if (f.oldPath && f.oldPath !== f.path) paths.push(f.oldPath);
  }
  return paths;
}

/** True when a keyboard event is aimed at something that edits text, where Ctrl+A must stay native. */
export function isEditableTarget(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return false;
  if (target.isContentEditable) return true;
  if (target instanceof HTMLTextAreaElement || target instanceof HTMLSelectElement) return true;
  if (target instanceof HTMLInputElement) {
    return !["checkbox", "radio", "button", "submit", "reset", "range", "color", "file"].includes(target.type);
  }
  return false;
}

/** Ctrl+A, or Cmd+A on macOS, with no other modifier. */
export function isSelectAllKey(event: KeyboardEvent): boolean {
  return (event.ctrlKey || event.metaKey) && !event.altKey && !event.shiftKey && (event.key === "a" || event.key === "A");
}

// ---- Grouping and sorting for display ------------------------------------------------------

/** How the Files panel groups its list. `status` is Staged, Unstaged, Untracked and Conflicted. */
export type GroupBy = "status" | "folder" | "none";
/**
 * `name` is by path; `status` is by kind of change (see `STATUS_RANK`), then path; `modified` is
 * newest first and only applies to the working tree.
 */
export type SortBy = "name" | "status" | "modified";

/** Order of the kinds of change when sorting by Git status. */
const STATUS_RANK: Record<FileState, number> = {
  added: 0,
  modified: 1,
  renamed: 2,
  deleted: 3,
  untracked: 4,
  conflict: 5,
  neutral: 6,
};

/** Commit mode has no index or working tree, so its status groups are the kinds of change. */
const COMMIT_STATUS_GROUPS: { key: string; title: string; states: FileState[] }[] = [
  { key: "added", title: "Added", states: ["added"] },
  { key: "modified", title: "Modified", states: ["modified"] },
  { key: "renamed", title: "Renamed or copied", states: ["renamed"] },
  { key: "deleted", title: "Deleted", states: ["deleted"] },
  { key: "other", title: "Other", states: ["conflict", "untracked", "neutral"] },
];

/**
 * Regroups and sorts the Files panel's sections for display. Items keep their ids, so the
 * selection and the Commit dialog see the same items whatever the grouping. `times` maps a path
 * to its last-modified time in milliseconds (null for a file that is gone); files without a time
 * sort after those with one.
 */
export function arrangeSections(
  sections: FileSection[],
  mode: "commit" | "worktree",
  groupBy: GroupBy,
  sortBy: SortBy,
  times: ReadonlyMap<string, number | null> | null,
): FileSection[] {
  const byDate = sortBy === "modified" && mode === "worktree" && times !== null;
  const compare = (a: FileItem, b: FileItem): number => {
    if (byDate) {
      const ta = times.get(a.path) ?? null;
      const tb = times.get(b.path) ?? null;
      if (ta !== tb) {
        if (ta === null) return 1;
        if (tb === null) return -1;
        return tb - ta;
      }
    }
    if (sortBy === "status" && a.state !== b.state) return STATUS_RANK[a.state] - STATUS_RANK[b.state];
    return a.path.localeCompare(b.path);
  };
  const sorted = (items: FileItem[]): FileItem[] => [...items].sort(compare);
  const all = sections.flatMap((s) => s.items);

  if (groupBy === "none") return [{ key: "all", title: "All files", items: sorted(all) }];

  if (groupBy === "folder") {
    const folders = new Map<string, FileItem[]>();
    for (const item of all) {
      const dir = splitPath(item.path).dir;
      const list = folders.get(dir);
      if (list) list.push(item);
      else folders.set(dir, [item]);
    }
    // Top-level files first, then folders alphabetically.
    return [...folders.entries()]
      .sort(([a], [b]) => (a === "" ? -1 : b === "" ? 1 : a.localeCompare(b)))
      .map(([dir, items]) => ({ key: `folder:${dir}`, title: dir === "" ? "Top level" : dir, items: sorted(items) }));
  }

  if (mode === "commit") {
    return COMMIT_STATUS_GROUPS.map((g) => ({
      key: g.key,
      title: g.title,
      items: sorted(all.filter((i) => g.states.includes(i.state))),
    }));
  }
  return sections.map((s) => ({ ...s, items: sorted(s.items) }));
}
