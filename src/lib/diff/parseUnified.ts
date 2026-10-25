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

const TOKEN_CAP = 500;

/**
 * Splits a line into tokens that re-join to the line: runs of letters, digits and underscores,
 * and every other character on its own. Splitting off punctuation and single spaces lets
 * `false})` and `false })` match on `false` and `})`, so only the added space is marked.
 */
function tokenise(text: string): string[] {
  return text.match(/[\p{L}\p{N}_]+|[^]/gu) ?? [];
}

/** A changed pair's two lines, cut into changed and unchanged text. */
export interface HighlightedPair {
  old: Segment[];
  new: Segment[];
  /**
   * Where words of the old line were removed with nothing put in their place: offsets into the
   * new line's text, in order. Side by side draws a cursor there on the new side.
   */
  removedAt: number[];
}

/**
 * Marks the tokens that differ between an old and a new line using a longest common
 * subsequence over their tokens. Lines with more than 500 tokens are returned unmarked, so a
 * pathological line cannot stall rendering. With `ignoreWhitespace`, as the diff's Ignore
 * whitespace button sets, spaces added or removed between unchanged words are left unmarked.
 */
export function highlightPair(oldText: string, newText: string, ignoreWhitespace = false): HighlightedPair {
  const a = tokenise(oldText);
  const b = tokenise(newText);
  if (a.length > TOKEN_CAP || b.length > TOKEN_CAP) {
    return { old: [{ text: oldText, changed: false }], new: [{ text: newText, changed: false }], removedAt: [] };
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
  const removedAt: number[] = [];
  const push = (list: Segment[], text: string, changed: boolean): void => {
    const last = list[list.length - 1];
    if (last && last.changed === changed) last.text += text;
    else list.push({ text, changed });
  };
  // Between two matched tokens: where in the new line the first removed word came, and whether
  // any word was added there. When whitespace is ignored, whitespace on its own counts as neither.
  const counts = (token: string): boolean => !ignoreWhitespace || token.trim().length > 0;
  let newOffset = 0;
  let removedFrom: number | null = null;
  let added = false;
  const takeOld = (token: string): void => {
    push(oldSeg, token, true);
    if (removedFrom === null && counts(token)) removedFrom = newOffset;
  };
  const takeNew = (token: string, changed: boolean): void => {
    push(newSeg, token, changed);
    newOffset += token.length;
    if (changed && counts(token)) added = true;
  };
  const settle = (): void => {
    if (removedFrom !== null && !added) removedAt.push(removedFrom);
    removedFrom = null;
    added = false;
  };
  let i = 0;
  let j = 0;
  while (i < n && j < m) {
    if (a[i] === b[j]) {
      settle();
      push(oldSeg, a[i] as string, false);
      takeNew(b[j] as string, false);
      i += 1;
      j += 1;
    } else if (((dp[i + 1] as Uint16Array)[j] as number) >= ((dp[i] as Uint16Array)[j + 1] as number)) {
      takeOld(a[i] as string);
      i += 1;
    } else {
      takeNew(b[j] as string, true);
      j += 1;
    }
  }
  while (i < n) {
    takeOld(a[i] as string);
    i += 1;
  }
  while (j < m) {
    takeNew(b[j] as string, true);
    j += 1;
  }
  settle();
  // Whitespace at either end of a changed run reads better unmarked, and so, when whitespace is
  // ignored, does a whitespace-only change between unchanged words. Otherwise that change is
  // marked: it is the whole difference. Unchanged whitespace between two changed runs is marked
  // with them, so the run reads as one block.
  const tidy = (list: Segment[]): Segment[] => {
    const softened = list.flatMap((s): Segment[] => {
      if (!s.changed || (!ignoreWhitespace && s.text.trim().length === 0)) return [s];
      const [, lead = "", core = "", trail = ""] = /^(\s*)(.*?)(\s*)$/s.exec(s.text) ?? [];
      return [
        { text: lead, changed: false },
        { text: core, changed: true },
        { text: trail, changed: false },
      ].filter((p) => p.text.length > 0);
    });
    const out: Segment[] = [];
    softened.forEach((s, k) => {
      const prev = out[out.length - 1];
      const bridges = !s.changed && s.text.trim().length === 0 && prev?.changed === true && softened[k + 1]?.changed === true;
      if (prev && (bridges || prev.changed === s.changed)) prev.text += s.text;
      else out.push({ ...s });
    });
    return out;
  };
  return { old: tidy(oldSeg), new: tidy(newSeg), removedAt };
}
