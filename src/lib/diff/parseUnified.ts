import type { DiffNote } from "@/lib/bindings";

export type DiffLineKind = "context" | "add" | "del" | "meta";

export interface DiffLine {
  kind: DiffLineKind;
  /** Line text without the leading marker character. */
  text: string;
  oldNo?: number;
  newNo?: number;
}

export interface DiffHunk {
  /** The full `@@ -a,b +c,d @@ heading` line. */
  header: string;
  oldStart: number;
  oldLines: number;
  newStart: number;
  newLines: number;
  lines: DiffLine[];
}

export interface DiffFile {
  oldPath: string | null;
  newPath: string | null;
  hunks: DiffHunk[];
  isBinary: boolean;
  /** `old mode` and `new mode`, when the change sets the file's mode. */
  oldMode: string | null;
  newMode: string | null;
}

export interface ParsedDiff {
  files: DiffFile[];
}

const HUNK_RE = /^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@/;

function stripPrefix(path: string): string | null {
  const trimmed = path.trim();
  if (trimmed === "/dev/null") return null;
  // `--- a/src/x.ts` or `+++ b/src/x.ts`; also handles `--- "a/with spaces"`.
  const unquoted = trimmed.startsWith('"') && trimmed.endsWith('"') ? trimmed.slice(1, -1) : trimmed;
  const tab = unquoted.indexOf("\t");
  const noTs = tab >= 0 ? unquoted.slice(0, tab) : unquoted;
  return noTs.replace(/^[ab]\//, "");
}

/**
 * Parses the text of `git diff` (unified format) into files, hunks and numbered lines.
 * `\ No newline at end of file` becomes a `meta` line. Binary files produce a file entry with
 * `isBinary: true` and no hunks.
 */
export function parseUnified(text: string): ParsedDiff {
  const files: DiffFile[] = [];
  let file: DiffFile | null = null;
  let hunk: DiffHunk | null = null;
  let oldNo = 0;
  let newNo = 0;

  const lines = text.split("\n");
  // A trailing newline yields one empty final element; drop it.
  if (lines.length > 0 && lines[lines.length - 1] === "") lines.pop();

  const startFile = (): DiffFile => {
    const f: DiffFile = { oldPath: null, newPath: null, hunks: [], isBinary: false, oldMode: null, newMode: null };
    files.push(f);
    hunk = null;
    return f;
  };

  for (const raw of lines) {
    const line = raw.endsWith("\r") ? raw.slice(0, -1) : raw;

    if (line.startsWith("diff --git ")) {
      file = startFile();
      continue;
    }

    if (hunk && file) {
      const marker = line.charAt(0);
      if (marker === "+") {
        hunk.lines.push({ kind: "add", text: line.slice(1), newNo });
        newNo += 1;
        continue;
      }
      if (marker === "-") {
        hunk.lines.push({ kind: "del", text: line.slice(1), oldNo });
        oldNo += 1;
        continue;
      }
      if (marker === " " || line === "") {
        hunk.lines.push({ kind: "context", text: line.slice(1), oldNo, newNo });
        oldNo += 1;
        newNo += 1;
        continue;
      }
      if (marker === "\\") {
        hunk.lines.push({ kind: "meta", text: line.slice(1).trim() });
        continue;
      }
      // Anything else ends the hunk (a new header or file follows).
      hunk = null;
    }

    if (line.startsWith("@@")) {
      const m = HUNK_RE.exec(line);
      if (!m) continue;
      if (!file) file = startFile();
      const h: DiffHunk = {
        header: line,
        oldStart: Number(m[1]),
        oldLines: m[2] === undefined ? 1 : Number(m[2]),
        newStart: Number(m[3]),
        newLines: m[4] === undefined ? 1 : Number(m[4]),
        lines: [],
      };
      file.hunks.push(h);
      hunk = h;
      oldNo = h.oldStart;
      newNo = h.newStart;
      continue;
    }

    if (line.startsWith("--- ")) {
      if (!file) file = startFile();
      file.oldPath = stripPrefix(line.slice(4));
      continue;
    }
    if (line.startsWith("+++ ")) {
      if (!file) file = startFile();
      file.newPath = stripPrefix(line.slice(4));
      continue;
    }
    if (line.startsWith("Binary files ") || line.startsWith("GIT binary patch")) {
      if (!file) file = startFile();
      file.isBinary = true;
      continue;
    }
    if (line.startsWith("rename from ")) {
      if (!file) file = startFile();
      file.oldPath = line.slice("rename from ".length);
      continue;
    }
    if (line.startsWith("rename to ")) {
      if (!file) file = startFile();
      file.newPath = line.slice("rename to ".length);
      continue;
    }
    if (line.startsWith("old mode ")) {
      if (!file) file = startFile();
      file.oldMode = line.slice("old mode ".length);
      continue;
    }
    if (line.startsWith("new mode ")) {
      if (!file) file = startFile();
      file.newMode = line.slice("new mode ".length);
      continue;
    }
    // `index`, `similarity index`, `deleted file mode` etc. carry nothing we render.
  }

  return { files };
}

const EXECUTABLE = "100755";

/**
 * What the diff panel says for a listed file whose diff has no lines: why there are none, where
 * that is known.
 */
export function noDiffMessage(note: DiffNote | null, parsed: ParsedDiff | null): string {
  if (note === "nestedRepository") {
    return "This folder is a Git repository of its own, so Git does not list its files here. Adding the folder would record it as a submodule.";
  }
  if (note === "submoduleUntracked") {
    return "The submodule has files it does not track. Git counts them as a change to the submodule, but they have no diff here; open the submodule to see them.";
  }
  const mode = parsed?.files.find((f) => f.oldMode !== null && f.newMode !== null);
  if (mode?.oldMode && mode.newMode) {
    const executable =
      mode.newMode === EXECUTABLE ? " The file is now executable." : mode.oldMode === EXECUTABLE ? " The file is no longer executable." : "";
    return `Only the file mode changed, from ${mode.oldMode} to ${mode.newMode}.${executable}`;
  }
  return "No differences to show.";
}

export interface Segment {
  text: string;
  changed: boolean;
}

const TOKEN_CAP = 200;

/** Splits a line into word and whitespace tokens, keeping the separators so they can be re-joined. */
function tokenise(text: string): string[] {
  return text.split(/(\s+)/).filter((t) => t.length > 0);
}

/**
 * Marks the tokens that differ between an old and a new line using a longest common
 * subsequence over whitespace-separated tokens. Lines with more than 200 tokens are returned
 * unmarked, so a pathological line cannot stall rendering.
 */
export function highlightPair(oldText: string, newText: string): { old: Segment[]; new: Segment[] } {
  const a = tokenise(oldText);
  const b = tokenise(newText);
  if (a.length > TOKEN_CAP || b.length > TOKEN_CAP) {
    return { old: [{ text: oldText, changed: false }], new: [{ text: newText, changed: false }] };
  }
  const n = a.length;
  const m = b.length;
  // dp[i][j] = LCS length of a[i..] and b[j..]
  const dp: Uint16Array[] = [];
  for (let i = 0; i <= n; i += 1) dp.push(new Uint16Array(m + 1));
  for (let i = n - 1; i >= 0; i -= 1) {
    const rowI = dp[i] as Uint16Array;
    const rowNext = dp[i + 1] as Uint16Array;
    for (let j = m - 1; j >= 0; j -= 1) {
      rowI[j] = a[i] === b[j] ? (rowNext[j + 1] as number) + 1 : Math.max(rowNext[j] as number, rowI[j + 1] as number);
    }
  }
  const oldSeg: Segment[] = [];
  const newSeg: Segment[] = [];
  const push = (list: Segment[], text: string, changed: boolean): void => {
    const last = list[list.length - 1];
    if (last && last.changed === changed) last.text += text;
    else list.push({ text, changed });
  };
  let i = 0;
  let j = 0;
  while (i < n && j < m) {
    if (a[i] === b[j]) {
      push(oldSeg, a[i] as string, false);
      push(newSeg, b[j] as string, false);
      i += 1;
      j += 1;
    } else if (((dp[i + 1] as Uint16Array)[j] as number) >= ((dp[i] as Uint16Array)[j + 1] as number)) {
      push(oldSeg, a[i] as string, true);
      i += 1;
    } else {
      push(newSeg, b[j] as string, true);
      j += 1;
    }
  }
  while (i < n) {
    push(oldSeg, a[i] as string, true);
    i += 1;
  }
  while (j < m) {
    push(newSeg, b[j] as string, true);
    j += 1;
  }
  // Whitespace-only changes between unchanged words read better unmarked.
  const soften = (list: Segment[]): Segment[] =>
    list.map((s) => (s.changed && s.text.trim().length === 0 ? { text: s.text, changed: false } : s));
  return { old: soften(oldSeg), new: soften(newSeg) };
}
