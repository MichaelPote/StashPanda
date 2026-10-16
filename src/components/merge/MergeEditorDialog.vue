<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, ref, shallowRef, watch } from "vue";
import { usePreferredDark } from "@vueuse/core";
import { ChevronDownIcon, ChevronUpIcon, LoaderCircleIcon, RotateCcwIcon } from "@lucide/vue";
import { EditorState, Text, type Extension } from "@codemirror/state";
import { EditorView, keymap, lineNumbers, type ViewUpdate } from "@codemirror/view";
import { defaultKeymap, history, historyKeymap } from "@codemirror/commands";
import type { Chunk } from "@codemirror/merge";
import { commands, type ConflictFile } from "@/lib/bindings";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import EmptyState from "@/components/shell/EmptyState.vue";
import FindBox from "@/components/diff/FindBox.vue";
import { useFind, type FindHit } from "@/composables/useFind";
import { findInText } from "@/lib/diff/find";
import { operationNames } from "@/lib/conflicts";
import { diffRowPx, diffTextStyle, resolveDiffFont } from "@/lib/diff/font";
import { bandPath } from "@/lib/diff/sideLayout";
import { themeFor } from "@/lib/diff/syntax";
import { errorMessage, IpcError, unwrap } from "@/lib/ipc";
import { blocksField, dismissedChunks, initialBlocks, placeholders, saveText, setChunkDismissed, setDismissed, setTaken, type MergeBlock } from "@/lib/merge/blocks";
import { buildChunks, chunkLines, mapLine, updateChunks, type LineChunk } from "@/lib/merge/chunks";
import { folds, foldsEffect, lineAbove, lineAtHeight, lineTop, type FoldRange } from "@/lib/merge/folds";
import { findMarks, findMarksEffect, lineMarks, lineMarksEffect, type FindMark, type LineMark } from "@/lib/merge/lines";
import { syntaxColours } from "@/lib/merge/syntax";
import { openWorktreeFile, runOp } from "@/lib/repoActions";
import { cn } from "@/lib/utils";
import { useDialogsStore } from "@/stores/dialogs";
import { useSessionsStore } from "@/stores/sessions";
import { useSettingsStore } from "@/stores/settings";

/**
 * The three-pane merge editor for one conflicted file. The left pane is the version on the
 * branch being built on and the right pane the incoming version, both read-only; the middle pane
 * is the result, editable, which is saved to the working tree. The gaps between the panes draw
 * each difference from the result as a curved band, with an arrow that copies that side's lines
 * into the result. The panes scroll together.
 *
 * The result starts as the working tree file with each conflict's markers taken out (see
 * `get_conflict_file`); a conflict still unresolved when the file is saved is written back with
 * the markers, so nothing is lost.
 */

const dialogs = useDialogsStore();
const sessions = useSessionsStore();
const settings = useSettingsStore();

const request = computed(() => dialogs.current("mergeEditor"));
/** The file shown; "Save, Resolve and Open Next" moves on to another without closing. */
const path = ref<string | null>(null);
const session = computed(() => (request.value ? (sessions.sessions.get(request.value.key) ?? null) : null));
const names = computed(() => {
  const s = session.value;
  const n = operationNames(s?.status ?? null, s?.refs ?? null, s?.stashConflict ?? null);
  // A stash put back by a pull's --autostash is not recorded; Git's own label says what it is.
  if (n.title === "" && file.value?.blocks[0]?.theirsLabel === "Stashed changes") return { ...n, title: `Putting stashed changes back on ${n.ours}`, theirs: "your stashed changes" };
  return n;
});

const file = shallowRef<ConflictFile | null>(null);
const loading = ref(false);
const saving = ref(false);
const error = ref<string | null>(null);
/** The working tree file's hash when it was read or last saved, to notice a change on disk. */
let fileHash = "";
let loadTicket = 0;

const leftHost = ref<HTMLElement | null>(null);
const middleHost = ref<HTMLElement | null>(null);
const rightHost = ref<HTMLElement | null>(null);
const leftGap = ref<HTMLElement | null>(null);
const rightGap = ref<HTMLElement | null>(null);

interface Panes {
  left: EditorView;
  middle: EditorView;
  right: EditorView;
  blocks: ReturnType<typeof blocksField>["field"];
}
let panes: Panes | null = null;
/** Redraws the gaps when the panes change size. Declared up here: the first load runs during setup. */
let resizeObserver: ResizeObserver | null = null;

/** The result's conflict blocks, the differences of each side from the result, and unsaved edits. */
const blocks = shallowRef<MergeBlock[]>([]);
const leftChunks = shallowRef<readonly Chunk[]>([]);
const rightChunks = shallowRef<readonly Chunk[]>([]);
const leftLines = shallowRef<LineChunk[]>([]);
const rightLines = shallowRef<LineChunk[]>([]);
const dirty = ref(false);
/** Counts edits to the result, so that Find looks again after one. */
const docVersion = ref(0);
const syncScroll = ref(true);
/** The differences dismissed with "×", by `chunkKey`, as the result pane's state holds them. */
const dismissed = shallowRef<ReadonlySet<string>>(new Set());
/** Fold long runs of unchanged lines; the folds shown again with their Show button, by key. */
const foldUnchanged = ref(true);
const shownFolds = ref(new Set<string>());

const unresolved = computed(() => blocks.value.filter((b) => !b.resolved));

const preferredDark = usePreferredDark();
const dark = computed(() => settings.theme === "dark" || (settings.theme === "system" && preferredDark.value));
const font = computed(() => resolveDiffFont(settings.settings?.diff));
const textStyle = computed(() => diffTextStyle(font.value));
const syntaxOn = computed(() => settings.settings?.diff?.syntaxHighlight !== false);

/** Width of each gap between two panes, where the curves and arrows are. */
const GAP_PX = 40;

// ---- Loading -------------------------------------------------------------------------------

watch(
  request,
  (r) => {
    path.value = r?.path ?? null;
  },
  { immediate: true },
);
// Immediate too: the watcher above has already set the path by the time this one starts.
watch(path, () => void load(), { immediate: true });

async function load(): Promise<void> {
  const r = request.value;
  const p = path.value;
  const ticket = (loadTicket += 1);
  destroyPanes();
  file.value = null;
  error.value = null;
  dirty.value = false;
  if (!r || !p) return;
  loading.value = true;
  try {
    const f = await unwrap(commands.getConflictFile(r.key, p));
    if (ticket !== loadTicket) return;
    file.value = f;
    fileHash = f.hash;
    if (f.refused === null) {
      await nextTick();
      if (ticket === loadTicket) createPanes(f);
    }
  } catch (e) {
    if (ticket === loadTicket) error.value = errorMessage(e);
  } finally {
    if (ticket === loadTicket) loading.value = false;
  }
}

const REFUSALS: Record<NonNullable<ConflictFile["refused"]>, string> = {
  notConflicted: "This file has no conflict any more.",
  deletedOnOneSide: "One side deleted this file, so there is no text to merge. Take one side's version instead.",
  binary: "This is a binary file, which the merge editor cannot show.",
  notUtf8: "This file is not UTF-8 text, which the merge editor cannot edit.",
  tooLarge: "This file is too large for the merge editor.",
  symlink: "This is a symbolic link, whose versions are where it points rather than text to merge. Take one side's version instead.",
  submodule: "This is a submodule, whose versions are commits rather than text to merge. Take one side's version instead.",
};

const refusal = computed(() => (file.value?.refused ? REFUSALS[file.value.refused] : null));

// ---- The three panes ---------------------------------------------------------------------

function paneTheme(): Extension {
  return EditorView.theme(
    {
      "&": { height: "100%", backgroundColor: "transparent", color: "var(--foreground)" },
      "&.cm-focused": { outline: "none" },
      // The app's thin scroll bars (the `scrollbar-thin` utility), not the system's wide ones.
      ".cm-scroller": {
        fontFamily: "inherit",
        lineHeight: "var(--diff-row-h)",
        overflow: "auto",
        scrollbarWidth: "thin",
        scrollbarColor: "color-mix(in oklch, var(--foreground) 25%, transparent) transparent",
      },
      ".cm-content": { caretColor: "var(--foreground)", padding: "0" },
      ".cm-line": { padding: "0 8px" },
      ".cm-gutters": { backgroundColor: "var(--color-diff-gutter)", color: "var(--color-gutter-fg)", border: "none" },
      ".cm-lineNumbers .cm-gutterElement": { padding: "0 6px 0 8px", minWidth: "4ch" },
    },
    { dark: dark.value },
  );
}

/** What every pane has: line numbers, the diff font, tabs, colours and line backgrounds. */
function paneExtensions(readOnly: boolean): Extension[] {
  const p = path.value ?? "";
  return [
    lineNumbers(),
    paneTheme(),
    EditorState.tabSize.of(font.value.tabWidth),
    EditorState.readOnly.of(readOnly),
    syntaxOn.value ? syntaxColours(p, themeFor(dark.value)) : [],
    lineMarks(),
    folds(),
    findMarks(),
    keymap.of(editorKeys),
  ];
}

function createPanes(f: ConflictFile): void {
  if (!leftHost.value || !middleHost.value || !rightHost.value) return;
  const tracked = blocksField(initialBlocks(Text.of(f.result.split("\n")), f.blocks));
  const middle = new EditorView({
    parent: middleHost.value,
    state: EditorState.create({
      doc: f.result,
      extensions: [
        ...paneExtensions(false),
        history(),
        keymap.of([...historyKeymap, ...defaultKeymap]),
        tracked.extension,
        placeholders(tracked.field, `Unresolved conflict: copy ${names.value.ours}'s or ${names.value.theirs}'s lines across with the arrows, or type here`, diffRowPx(font.value)),
        EditorView.updateListener.of(onMiddleUpdate),
      ],
    }),
  });
  const left = new EditorView({ parent: leftHost.value, state: EditorState.create({ doc: f.ours ?? "", extensions: paneExtensions(true) }) });
  const right = new EditorView({ parent: rightHost.value, state: EditorState.create({ doc: f.theirs ?? "", extensions: paneExtensions(true) }) });
  panes = { left, middle, right, blocks: tracked.field };
  blocks.value = middle.state.field(tracked.field);
  leftChunks.value = buildChunks(left.state.doc, middle.state.doc);
  rightChunks.value = buildChunks(right.state.doc, middle.state.doc);
  refreshLines();
  for (const [name, view] of [["left", left], ["middle", middle], ["right", right]] as const) {
    view.scrollDOM.addEventListener("scroll", () => onPaneScroll(name), { passive: true });
  }
  resizeObserver = new ResizeObserver(() => scheduleDraw());
  resizeObserver.observe(middle.dom);
  scheduleDraw();
  // Start at the first conflict, with the keyboard in the result.
  if (unresolved.value.length > 0) void nextTick(() => goToConflict(1));
}

function destroyPanes(): void {
  resizeObserver?.disconnect();
  resizeObserver = null;
  if (!panes) return;
  panes.left.destroy();
  panes.middle.destroy();
  panes.right.destroy();
  panes = null;
  blocks.value = [];
  leftChunks.value = [];
  rightChunks.value = [];
  leftLines.value = [];
  rightLines.value = [];
  // Both are keyed by line numbers, which mean nothing in the next file.
  dismissed.value = new Set();
  shownFolds.value = new Set();
}

onBeforeUnmount(destroyPanes);

function onMiddleUpdate(update: ViewUpdate): void {
  if (!panes) return;
  if (update.docChanged) {
    dirty.value = true;
    docVersion.value += 1;
    leftChunks.value = updateChunks(leftChunks.value, panes.left.state.doc, update.state.doc, update.changes);
    rightChunks.value = updateChunks(rightChunks.value, panes.right.state.doc, update.state.doc, update.changes);
  }
  const next = update.state.field(panes.blocks);
  const nextDismissed = update.state.field(dismissedChunks);
  if (next !== blocks.value || nextDismissed !== dismissed.value || update.docChanged) {
    blocks.value = next;
    dismissed.value = nextDismissed;
    refreshLines();
  }
}

/** A block's lines in the result, end exclusive. */
function blockLines(b: MergeBlock): [number, number] {
  if (!panes) return [0, 0];
  const doc = panes.middle.state.doc;
  const first = doc.lineAt(b.from).number - 1;
  if (b.to <= b.from) return [first, first];
  return [first, doc.lineAt(Math.max(b.from, b.to - 1)).number];
}

/** True when a difference's result lines meet a conflict block that is still unresolved. */
function isConflict(c: LineChunk): boolean {
  return unresolved.value.some((b) => {
    const [s, e] = blockLines(b);
    return s === e || c.b0 === c.b1 ? c.b0 <= e && c.b1 >= s : c.b0 < e && c.b1 > s;
  });
}

/** Names a difference by its side and that side's lines, which never change. */
function chunkKey(side: Side, c: LineChunk): string {
  return `${side}:${c.a0}-${c.a1}`;
}

/** Recomputes the line ranges of the differences, every pane's line backgrounds, and the folds. */
function refreshLines(): void {
  if (!panes) return;
  const { left, middle, right } = panes;
  leftLines.value = leftChunks.value.map((c) => chunkLines(c, left.state.doc, middle.state.doc));
  rightLines.value = rightChunks.value.map((c) => chunkLines(c, right.state.doc, middle.state.doc));
  const kind = (c: LineChunk): LineMark["kind"] => (isConflict(c) ? "conflict" : "change");
  // A dismissed difference keeps no background, on its side or in the result.
  const kept = (side: Side, lines: LineChunk[]): LineChunk[] => lines.filter((c) => !dismissed.value.has(chunkKey(side, c)));
  const leftKept = kept("left", leftLines.value);
  const rightKept = kept("right", rightLines.value);
  const leftMarks = leftKept.map((c) => ({ from: c.a0, to: c.a1, kind: kind(c) }));
  const rightMarks = rightKept.map((c) => ({ from: c.a0, to: c.a1, kind: kind(c) }));
  const middleMarks: LineMark[] = [
    ...[...leftKept, ...rightKept].map((c) => ({ from: c.b0, to: c.b1, kind: kind(c) })),
    ...unresolved.value.map((b) => {
      const [from, to] = blockLines(b);
      return { from, to, kind: "conflict" as const };
    }),
  ];
  // Conflict last, so it wins where a line has both.
  middleMarks.sort((x, y) => (x.kind === y.kind ? 0 : x.kind === "conflict" ? 1 : -1));
  left.dispatch({ effects: lineMarksEffect(left.state.doc, leftMarks) });
  right.dispatch({ effects: lineMarksEffect(right.state.doc, rightMarks) });
  middle.dispatch({ effects: lineMarksEffect(middle.state.doc, dedupeMarks(middleMarks)) });
  refreshFolds();
  scheduleDraw();
}

// ---- Folding unchanged lines -------------------------------------------------------------

/** The folds each pane has now, to open one that hides a Find match. */
let lastFolds: Record<PaneName, FoldRange[]> = { left: [], middle: [], right: [] };

/** Lines kept on show around every difference and conflict. */
const FOLD_CONTEXT = 3;
/** The fewest lines worth folding: a shorter run stays on show. */
const FOLD_MIN = 4;

/**
 * Folds each run of lines the same in all three panes, apart from a few lines of context next to
 * the differences. A run is found in the result and mapped to each side, where it has the same
 * number of lines, and named by where it starts on the left, which never changes.
 */
function refreshFolds(): void {
  if (!panes) return;
  const { left, middle, right } = panes;
  const n = middle.state.doc.lines;
  const ranges = { left: [] as FoldRange[], middle: [] as FoldRange[], right: [] as FoldRange[] };
  if (foldUnchanged.value) {
    const busy = new Uint8Array(n + 1);
    const keep = (from: number, to: number): void => {
      for (let line = Math.max(0, from - FOLD_CONTEXT); line < Math.min(n, Math.max(to, from + 1) + FOLD_CONTEXT); line += 1) busy[line] = 1;
    };
    for (const c of [...leftLines.value, ...rightLines.value]) keep(c.b0, c.b1);
    for (const b of unresolved.value) keep(...blockLines(b));
    let start = -1;
    for (let line = 0; line <= n; line += 1) {
      const free = line < n && busy[line] === 0;
      if (free && start < 0) start = line;
      if (!free && start >= 0) {
        if (line - start >= FOLD_MIN) {
          const count = line - start;
          const l0 = Math.round(mapLine(leftLines.value, start, false));
          const r0 = Math.round(mapLine(rightLines.value, start, false));
          const key = `${l0}`;
          if (!shownFolds.value.has(key)) {
            ranges.middle.push({ key, from: start, to: line });
            ranges.left.push({ key, from: l0, to: l0 + count });
            ranges.right.push({ key, from: r0, to: r0 + count });
          }
        }
        start = -1;
      }
    }
  }
  lastFolds = ranges;
  const row = diffRowPx(font.value);
  for (const [view, list] of [[left, ranges.left], [middle, ranges.middle], [right, ranges.right]] as const) {
    view.dispatch({ effects: foldsEffect(view.state.doc, list, row, showFold) });
  }
}

function showFold(key: string): void {
  shownFolds.value = new Set([...shownFolds.value, key]);
  refreshFolds();
  scheduleDraw();
}

watch(foldUnchanged, () => {
  shownFolds.value = new Set();
  refreshFolds();
  scheduleDraw();
});

/** One mark per line: a conflict wins over a plain difference. */
function dedupeMarks(marks: LineMark[]): LineMark[] {
  const kinds = new Map<number, LineMark["kind"]>();
  for (const m of marks) for (let line = m.from; line < m.to; line += 1) if (kinds.get(line) !== "conflict") kinds.set(line, m.kind);
  return [...kinds.entries()].sort((a, b) => a[0] - b[0]).map(([line, kind]) => ({ from: line, to: line + 1, kind }));
}

// ---- Curves and arrows in the gaps ---------------------------------------------------------

interface GapShape {
  key: string;
  d: string;
  kind: LineMark["kind"];
  /** Dismissed with "×": drawn faded, with no arrow. */
  dismissed: boolean;
  /** Where the arrow sits, from the top of the gap, and which difference it takes. */
  arrowY: number;
  index: number;
}

/** A mark on a pane's scroll bar strip, in percent of the strip's height. */
interface RulerMark {
  key: string;
  top: number;
  height: number;
  kind: LineMark["kind"];
  line: number;
}

type PaneName = "left" | "middle" | "right";
const rulers = shallowRef<Record<PaneName, { marks: RulerMark[]; right: number }>>({
  left: { marks: [], right: 0 },
  middle: { marks: [], right: 0 },
  right: { marks: [], right: 0 },
});

/** Each pane's scroll bar marks: its differences and, in the result, its unresolved conflicts. */
function drawRulers(): void {
  if (!panes) return;
  const ranges: Record<PaneName, { from: number; to: number; kind: LineMark["kind"] }[]> = { left: [], middle: [], right: [] };
  for (const [side, lines] of [["left", leftLines.value], ["right", rightLines.value]] as const) {
    for (const c of lines) {
      if (dismissed.value.has(chunkKey(side, c))) continue;
      const kind: LineMark["kind"] = isConflict(c) ? "conflict" : "change";
      ranges[side].push({ from: c.a0, to: c.a1, kind });
      ranges.middle.push({ from: c.b0, to: c.b1, kind });
    }
  }
  for (const b of unresolved.value) {
    const [from, to] = blockLines(b);
    ranges.middle.push({ from, to, kind: "conflict" });
  }
  const next = { ...rulers.value };
  for (const name of ["left", "middle", "right"] as const) {
    const view = panes[name];
    // A file shorter than its pane maps to the pane's height, so each mark sits level with its lines.
    const total = Math.max(1, view.contentHeight, view.scrollDOM.clientHeight);
    const marks = ranges[name]
      // Conflicts last, so they are drawn over a plain difference at the same place.
      .sort((x, y) => (x.kind === y.kind ? 0 : x.kind === "conflict" ? 1 : -1))
      .map((r, i): RulerMark => {
        const top = r.from === r.to ? lineAbove(view, r.from) : lineTop(view, r.from);
        const bottom = r.from === r.to ? lineTop(view, r.from) : lineAbove(view, r.to);
        return { key: `${i}`, top: (100 * top) / total, height: (100 * Math.max(0, bottom - top)) / total, kind: r.kind, line: r.from };
      });
    next[name] = { marks, right: view.scrollDOM.offsetWidth - view.scrollDOM.clientWidth };
  }
  rulers.value = next;
}

/** Scrolls a pane so that `line` is in the middle; the other panes follow. */
function scrollToLine(name: PaneName, line: number): void {
  const view = panes?.[name];
  if (!view) return;
  const doc = view.state.doc;
  const pos = doc.line(Math.min(doc.lines, line + 1)).from;
  view.dispatch({ effects: EditorView.scrollIntoView(pos, { y: "center" }) });
}

const leftShapes = shallowRef<GapShape[]>([]);
const rightShapes = shallowRef<GapShape[]>([]);
const gapHeight = ref(0);
let drawFrame = 0;

function scheduleDraw(): void {
  if (drawFrame !== 0) return;
  drawFrame = requestAnimationFrame(() => {
    drawFrame = 0;
    draw();
  });
}

/** The top of line `line` (counted from 0) of a pane, from the top of `gap`, folds included. */
function lineY(view: EditorView, line: number, gapTop: number): number {
  return view.documentTop + lineTop(view, line) - gapTop;
}

/**
 * The top and bottom of lines `[from, to)` of a pane, from the top of `gap`. Lines end where the
 * next line's placeholder starts; no lines at all span the placeholder at that place, if any,
 * so an unresolved conflict with no lines in the result is drawn reaching its placeholder row.
 */
function rangeY(view: EditorView, from: number, to: number, gapTop: number): [number, number] {
  if (from === to) return [view.documentTop + lineAbove(view, from) - gapTop, lineY(view, from, gapTop)];
  return [lineY(view, from, gapTop), view.documentTop + lineAbove(view, to) - gapTop];
}

function draw(): void {
  if (!panes || !leftGap.value || !rightGap.value) return;
  const { left, middle, right } = panes;
  const lg = leftGap.value.getBoundingClientRect();
  const rg = rightGap.value.getBoundingClientRect();
  gapHeight.value = lg.height;
  const shapes = (lines: LineChunk[], side: EditorView, sideOnLeft: boolean, gapTop: number, height: number): GapShape[] => {
    const sideName: Side = sideOnLeft ? "left" : "right";
    const out: GapShape[] = [];
    lines.forEach((c, index) => {
      const [st, sb] = rangeY(side, c.a0, c.a1, gapTop);
      const [mt, mb] = rangeY(middle, c.b0, c.b1, gapTop);
      if (Math.max(sb, mb) < -40 || Math.min(st, mt) > height + 40) return;
      const kind: LineMark["kind"] = isConflict(c) ? "conflict" : "change";
      const d = sideOnLeft ? bandPath(GAP_PX, st, sb, mt, mb) : bandPath(GAP_PX, mt, mb, st, sb);
      out.push({ key: `${index}`, d, kind, dismissed: dismissed.value.has(chunkKey(sideName, c)), arrowY: st, index });
    });
    return out;
  };
  leftShapes.value = shapes(leftLines.value, left, true, lg.top, lg.height);
  rightShapes.value = shapes(rightLines.value, right, false, rg.top, rg.height);
  drawRulers();
}

// ---- Taking a side's lines -----------------------------------------------------------------

type Side = "left" | "right";

/** Copies one side's lines of difference `index` into the result, over the result's lines, or below them. */
function take(side: Side, index: number, below = false): void {
  if (!panes) return;
  const chunk = (side === "left" ? leftChunks.value : rightChunks.value)[index];
  if (!chunk) return;
  const source = side === "left" ? panes.left : panes.right;
  const text = source.state.doc.sliceString(chunk.fromA, chunk.toA);
  const changes = below ? { from: chunk.toB, insert: text } : { from: chunk.fromB, to: chunk.toB, insert: text };
  // Every conflict block the difference meets counts as taken, even when its text stays the same.
  const met = blocks.value.filter((b) => (b.from === b.to ? chunk.fromB <= b.from && chunk.toB >= b.from : chunk.fromB < b.to && chunk.toB > b.from));
  panes.middle.dispatch({ changes, effects: met.map((b) => setTaken.of({ id: b.id, taken: true })), userEvent: "input.take" });
  panes.middle.focus();
}

/**
 * Dismisses difference `index` of one side: the user does not want those lines. Its arrow goes
 * and its band fades; a conflict block it meets counts that side as dismissed, and is resolved
 * once both sides are, keeping the result as it is. Undo brings it back.
 */
function dismiss(side: Side, index: number): void {
  if (!panes) return;
  const chunk = (side === "left" ? leftChunks.value : rightChunks.value)[index];
  const lines = (side === "left" ? leftLines.value : rightLines.value)[index];
  if (!chunk || !lines) return;
  const met = blocks.value.filter((b) => (b.from === b.to ? chunk.fromB <= b.from && chunk.toB >= b.from : chunk.fromB < b.to && chunk.toB > b.from));
  panes.middle.dispatch({
    effects: [setChunkDismissed.of({ key: chunkKey(side, lines), dismissed: true }), ...met.map((b) => setDismissed.of({ id: b.id, side, dismissed: true }))],
    userEvent: "select.dismiss",
  });
  panes.middle.focus();
}

/** Resolves every unresolved block with one side's version. */
function takeAll(side: Side): void {
  if (!panes) return;
  const list = unresolved.value;
  if (list.length === 0) return;
  panes.middle.dispatch({
    changes: list.map((b) => ({ from: b.from, to: b.to, insert: side === "left" ? b.source.ours : b.source.theirs })),
    effects: list.map((b) => setTaken.of({ id: b.id, taken: true })),
    userEvent: "input.take",
  });
}

/** The difference of one side that holds the result line the cursor is on. */
function chunkAtCursor(side: Side): number {
  if (!panes) return -1;
  const line = panes.middle.state.doc.lineAt(panes.middle.state.selection.main.head).number - 1;
  const lines = side === "left" ? leftLines.value : rightLines.value;
  return lines.findIndex((c) => (c.b0 === c.b1 ? c.b0 === line : c.b0 <= line && line < c.b1));
}

function takeAtCursor(side: Side): boolean {
  const index = chunkAtCursor(side);
  if (index >= 0) take(side, index);
  return true;
}

/** Moves the cursor to the next (1) or previous (-1) unresolved conflict, wrapping around. */
function goToConflict(direction: 1 | -1): boolean {
  if (!panes) return true;
  const list = [...unresolved.value].sort((a, b) => a.from - b.from);
  if (list.length === 0) return true;
  const head = panes.middle.state.selection.main.head;
  const target =
    direction === 1 ? (list.find((b) => b.from > head) ?? list[0]) : ([...list].reverse().find((b) => b.from < head) ?? list[list.length - 1]);
  if (!target) return true;
  panes.middle.dispatch({ selection: { anchor: target.from }, effects: EditorView.scrollIntoView(target.from, { y: "center" }) });
  panes.middle.focus();
  return true;
}

/** Keys in every pane. Alt+Up and Alt+Down are left to CodeMirror, which moves lines with them. */
const editorKeys = [
  { key: "F7", run: () => goToConflict(1) },
  { key: "Shift-F7", run: () => goToConflict(-1) },
  { key: "Alt-Shift-ArrowLeft", run: () => takeAtCursor("left") },
  { key: "Alt-Shift-ArrowRight", run: () => takeAtCursor("right") },
  {
    key: "Mod-s",
    run: () => {
      void save(false);
      return true;
    },
  },
];

// ---- Scrolling together --------------------------------------------------------------------

/** Panes this code just scrolled, whose own scroll event must not lead the others. */
const following = new Set<string>();

function onPaneScroll(name: "left" | "middle" | "right"): void {
  scheduleDraw();
  if (following.delete(name) || !syncScroll.value || !panes) return;
  const view = panes[name];
  const rect = view.scrollDOM.getBoundingClientRect();
  // The line at the middle of the pane scrolled; the others are brought level with it there.
  const line = lineAtHeight(view, rect.top + view.scrollDOM.clientHeight / 2 - view.documentTop);
  let middleLine = line;
  if (name === "left") middleLine = mapLine(leftLines.value, line, true);
  if (name === "right") middleLine = mapLine(rightLines.value, line, true);
  const targets: [string, EditorView, number][] = [];
  if (name !== "middle") targets.push(["middle", panes.middle, middleLine]);
  if (name !== "left") targets.push(["left", panes.left, mapLine(leftLines.value, middleLine, false)]);
  if (name !== "right") targets.push(["right", panes.right, mapLine(rightLines.value, middleLine, false)]);
  for (const [target, other, otherLine] of targets) {
    const el = other.scrollDOM;
    const otherRect = el.getBoundingClientRect();
    const delta = other.documentTop + lineTop(other, otherLine) - (otherRect.top + el.clientHeight / 2);
    const before = el.scrollTop;
    const top = Math.max(0, before + delta);
    if (Math.abs(top - before) > 0.5) {
      following.add(target);
      el.scrollTop = top;
      if (el.scrollTop === before) following.delete(target);
    }
    if (Math.abs(el.scrollLeft - view.scrollDOM.scrollLeft) > 0.5) {
      following.add(target);
      el.scrollLeft = view.scrollDOM.scrollLeft;
    }
  }
}

// ---- Find (Ctrl+F) -------------------------------------------------------------------------
//
// The Find box of the diff view and Blame (`useFind`, `FindBox.vue`), over all three panes at
// once: the matches are stepped through from the top of the file down, a side's lines placed by
// the result line they match, and the pane holding the current one scrolls to it.

const PANE_ORDER: Record<PaneName, number> = { left: 0, middle: 1, right: 2 };

interface MergeFindHit extends FindHit {
  pane: PaneName;
  line: number;
}

function collectMatches(query: string, max: number): MergeFindHit[] {
  void docVersion.value;
  if (!panes) return [];
  const out: MergeFindHit[] = [];
  for (const pane of ["left", "middle", "right"] as const) {
    const doc = panes[pane].state.doc;
    for (let i = 0; i < doc.lines && out.length < max; i += 1) {
      for (const [start, end] of findInText(doc.line(i + 1).text, query)) {
        if (out.length >= max) break;
        const row = pane === "middle" ? i : mapLine(pane === "left" ? leftLines.value : rightLines.value, i, true);
        out.push({ key: `${pane}:${i}`, start, end, row, pane, line: i });
      }
    }
  }
  return out.sort((x, y) => x.row - y.row || PANE_ORDER[x.pane] - PANE_ORDER[y.pane] || x.start - y.start);
}

/** Scrolls the pane holding a match to it, first opening the fold that hides it, if any. */
function revealMatch(hit: MergeFindHit): void {
  const fold = lastFolds[hit.pane].find((f) => f.from <= hit.line && hit.line < f.to);
  if (fold) showFold(fold.key);
  void nextTick(() => {
    const view = panes?.[hit.pane];
    if (!view) return;
    const pos = view.state.doc.line(hit.line + 1).from + hit.start;
    view.dispatch({ effects: EditorView.scrollIntoView(pos, { y: "center", x: "center" }) });
  });
}

const find = useFind<MergeFindHit>({
  collect: collectMatches,
  topRow: () => {
    const view = panes?.middle;
    if (!view) return 0;
    return lineAtHeight(view, view.scrollDOM.getBoundingClientRect().top - view.documentTop);
  },
  reveal: revealMatch,
  refocus: () => panes?.middle.focus(),
  resetOn: [file],
});

// Draw the matches in their panes whenever they or the current one change.
watch([find.matches, find.current], () => {
  const p = panes;
  if (!p) return;
  const marks: Record<PaneName, FindMark[]> = { left: [], middle: [], right: [] };
  find.matches.value.forEach((m, n) => {
    const from = p[m.pane].state.doc.line(m.line + 1).from;
    marks[m.pane].push({ from: from + m.start, to: from + m.end, current: n === find.current.value });
  });
  for (const pane of ["left", "middle", "right"] as const) p[pane].dispatch({ effects: findMarksEffect(marks[pane]) });
});

function onEditorKeydown(event: KeyboardEvent): void {
  if (panes) find.onKeydown(event);
}

// ---- Saving --------------------------------------------------------------------------------

/** Writes the result to the working tree; with `resolve`, then marks the file resolved (git add). */
async function save(resolve: boolean, openNext = false): Promise<void> {
  const r = request.value;
  const p = path.value;
  const f = file.value;
  if (!panes || !r || !p || !f || saving.value) return;
  if (resolve && unresolved.value.length > 0) return;
  saving.value = true;
  try {
    const content = saveText(panes.middle.state, blocks.value, { crlf: f.crlf, bom: f.bom, markerSize: f.markerSize });
    try {
      fileHash = await unwrap(commands.writeWorktreeFile(r.key, p, content, fileHash));
    } catch (e) {
      if (e instanceof IpcError && e.error.kind === "changedOnDisk") {
        const reload = await dialogs.open("confirm", {
          title: "The file changed on disk",
          message: `${p} changed since the merge editor read it. Reload it, losing the edits made here, or keep editing and save again later?`,
          confirmLabel: "Reload",
          destructive: true,
        });
        if (reload) await load();
        return;
      }
      throw e;
    }
    dirty.value = false;
    if (!resolve) {
      sessions.notifySuccess(`Saved ${p}.`);
      return;
    }
    const result = await runOp(r.key, () => commands.opStage(r.key, [p]));
    if (!result.ok) {
      sessions.notify(`Saved ${p}, but marking it resolved failed.`);
      return;
    }
    sessions.notifySuccess(`Saved ${p} and marked it as resolved.`);
    const next = openNext ? nextConflicted(p) : null;
    if (next) path.value = next;
    else close();
  } catch (e) {
    sessions.notify(errorMessage(e));
  } finally {
    saving.value = false;
  }
}

/** Another conflicted file both sides still have, to open after this one. */
function nextConflicted(current: string): string | null {
  const entries = session.value?.status?.entries ?? [];
  const candidates = entries.filter((e) => e.conflicted && e.path !== current && ["UU", "AA"].includes(`${e.x}${e.y}`)).map((e) => e.path);
  return candidates.sort()[0] ?? null;
}

async function restoreGitMerge(): Promise<void> {
  const r = request.value;
  const p = path.value;
  if (!r || !p) return;
  const ok = await dialogs.open("confirm", {
    title: "Start again from Git's merge?",
    message: `This writes Git's merge of ${p} again, with its conflict markers, and throws away every change made to the file since the conflict, here or in another editor.`,
    confirmLabel: "Start Again",
    destructive: true,
  });
  if (!ok) return;
  try {
    const result = await runOp(r.key, () => commands.opRestoreConflict(r.key, p));
    if (result.ok) await load();
    else sessions.notify(`Starting ${p} again from Git's merge failed.`);
  } catch (e) {
    sessions.notify(errorMessage(e));
  }
}

// ---- Closing -------------------------------------------------------------------------------

function close(): void {
  destroyPanes();
  dialogs.resolve("mergeEditor", null);
}

async function onOpenChange(open: boolean): Promise<void> {
  if (open) return;
  if (dirty.value) {
    const discard = await dialogs.open("confirm", {
      title: "Discard the edits?",
      message: `The result for ${path.value ?? "this file"} has changes that are not saved.`,
      confirmLabel: "Discard",
      destructive: true,
    });
    if (!discard) return;
  }
  close();
}

const conflictCount = computed(() => {
  const n = unresolved.value.length;
  const total = blocks.value.length;
  if (total === 0) return "No conflict blocks";
  return n === 0 ? `All ${total} resolved` : `${n} of ${total} ${total === 1 ? "conflict" : "conflicts"} left`;
});

const resolveBlocked = computed(() => {
  if (!file.value || file.value.refused !== null) return "There is no result to save.";
  return unresolved.value.length > 0 ? "Resolve every conflict first, or save it with its markers and finish later." : null;
});
</script>

<template>
  <Dialog :open="request !== null" @update:open="onOpenChange">
    <DialogContent v-if="request && path" class="h-[90vh] sm:max-w-[95vw]" size-key="mergeEditor">
      <DialogHeader>
        <DialogTitle>Merge {{ path }}</DialogTitle>
        <DialogDescription>
          {{ names.title || "Resolve the conflict" }}. Copy lines from either side with the arrows between the panes, or edit the result in the middle.
        </DialogDescription>
      </DialogHeader>

      <div class="-mx-6 -my-5 flex min-h-0 flex-1 flex-col bg-window p-1.5" data-dialog-grow>
        <EmptyState v-if="error" :message="error" class="text-destructive" />
        <EmptyState v-else-if="loading && !file" message="Reading the conflict…" />
        <EmptyState v-else-if="refusal" :message="refusal">
          <Button type="button" variant="outline" size="sm" @click="openWorktreeFile(request.key, path)">Open in Your Editor</Button>
        </EmptyState>

        <section v-else-if="file" class="area flex min-h-0 flex-1 flex-col" :style="textStyle" data-merge-editor @keydown="onEditorKeydown">
          <!-- Toolbar -->
          <div class="flex shrink-0 flex-wrap items-center gap-1.5 border-b border-border-soft px-2 py-1 font-sans text-[0.7rem]">
            <button type="button" class="diff-action" title="Previous conflict (Shift+F7)" :disabled="unresolved.length === 0" @click="goToConflict(-1)">
              <ChevronUpIcon class="size-3" />
            </button>
            <button type="button" class="diff-action" title="Next conflict (F7)" :disabled="unresolved.length === 0" @click="goToConflict(1)">
              <ChevronDownIcon class="size-3" />
            </button>
            <span :class="cn('tabular-nums', unresolved.length > 0 ? 'text-foreground' : 'text-muted-foreground')" data-merge-count>{{ conflictCount }}</span>
            <span class="mx-1 h-3 w-px bg-border-soft" aria-hidden="true" />
            <button type="button" class="diff-action" :disabled="unresolved.length === 0" @click="takeAll('left')">Take {{ names.ours }} for All Conflicts</button>
            <button type="button" class="diff-action" :disabled="unresolved.length === 0" @click="takeAll('right')">Take {{ names.theirs }} for All Conflicts</button>
            <span class="ml-auto flex items-center gap-1.5">
              <label class="inline-flex items-center gap-1 text-muted-foreground">
                <input v-model="foldUnchanged" type="checkbox" class="size-3 accent-primary" data-merge-fold-toggle />
                Hide unchanged lines
              </label>
              <label class="inline-flex items-center gap-1 text-muted-foreground">
                <input v-model="syncScroll" type="checkbox" class="size-3 accent-primary" />
                Scroll together
              </label>
              <button type="button" class="diff-action" title="Write Git's merge of this file again, with its conflict markers" @click="restoreGitMerge">
                <RotateCcwIcon class="mr-1 size-3" />
                Start Again from Git's Merge
              </button>
            </span>
          </div>

          <!-- Pane headers -->
          <div class="merge-columns shrink-0 border-b border-border-soft font-sans text-xs" :style="{ '--merge-gap': `${GAP_PX}px` }">
            <div class="truncate px-2 py-1" :title="names.ours" data-merge-side="left"><span class="font-semibold">{{ names.ours }}</span></div>
            <div />
            <div class="truncate px-2 py-1"><span class="font-semibold">Result</span> <span class="text-muted-foreground">saved to the working tree</span></div>
            <div />
            <div class="truncate px-2 py-1" :title="names.theirs" data-merge-side="right"><span class="font-semibold">{{ names.theirs }}</span></div>
          </div>

          <!-- Panes and gaps -->
          <div class="merge-columns relative min-h-0 flex-1" :style="{ '--merge-gap': `${GAP_PX}px` }">
            <!-- The Find box (Ctrl+F), over the result pane's top right corner. -->
            <FindBox
              v-if="find.open.value"
              :ref="(el) => (find.box.value = el as InstanceType<typeof FindBox> | null)"
              v-model="find.query.value"
              class="absolute top-1.5 right-5 z-20"
              :summary="find.summary.value"
              :can-step="find.matches.value.length > 0"
              placeholder="Find in all three versions"
              @step="find.step"
              @close="find.close"
            />
            <div class="relative min-h-0 min-w-0">
              <div ref="leftHost" class="h-full overflow-hidden" data-merge-pane="left" />
              <!-- Marks for the pane's differences and conflicts, beside its scroll bar: click one to go there. -->
              <div class="merge-ruler" :style="{ right: `${rulers.left.right}px` }" aria-hidden="true" data-merge-ruler="left">
                <button
                  v-for="m in rulers.left.marks"
                  :key="m.key"
                  type="button"
                  tabindex="-1"
                  :class="`merge-ruler-${m.kind}`"
                  :style="{ top: `${m.top}%`, height: `max(2px, ${m.height}%)` }"
                  @click="scrollToLine('left', m.line)"
                />
              </div>
            </div>
            <div ref="leftGap" class="relative min-h-0 overflow-hidden" data-merge-gap="left">
              <svg class="pointer-events-none absolute inset-0" :width="GAP_PX" :height="gapHeight" aria-hidden="true">
                <path v-for="s in leftShapes" :key="s.key" :d="s.d" :class="[`merge-curve-${s.kind}`, s.dismissed && 'merge-curve-dismissed']" />
              </svg>
              <template v-for="s in leftShapes" :key="`a${s.key}`">
                <template v-if="!s.dismissed">
                  <button
                    type="button"
                    class="merge-arrow left-0.5"
                    :style="{ top: `${s.arrowY}px` }"
                    :title="`Copy these lines from ${names.ours} into the result (Ctrl+click to add them below)`"
                    data-merge-take="left"
                    @click="take('left', s.index, $event.ctrlKey || $event.metaKey)"
                  >
                    »
                  </button>
                  <button
                    type="button"
                    class="merge-arrow merge-dismiss left-5"
                    :style="{ top: `${s.arrowY}px` }"
                    :title="`Leave these lines from ${names.ours} out (undo brings them back)`"
                    data-merge-dismiss="left"
                    @click="dismiss('left', s.index)"
                  >
                    ×
                  </button>
                </template>
              </template>
            </div>
            <div class="relative min-h-0 min-w-0">
              <div ref="middleHost" class="h-full overflow-hidden" data-merge-pane="middle" />
              <!-- Marks for the pane's differences and conflicts, beside its scroll bar: click one to go there. -->
              <div class="merge-ruler" :style="{ right: `${rulers.middle.right}px` }" aria-hidden="true" data-merge-ruler="middle">
                <button
                  v-for="m in rulers.middle.marks"
                  :key="m.key"
                  type="button"
                  tabindex="-1"
                  :class="`merge-ruler-${m.kind}`"
                  :style="{ top: `${m.top}%`, height: `max(2px, ${m.height}%)` }"
                  @click="scrollToLine('middle', m.line)"
                />
              </div>
            </div>
            <div ref="rightGap" class="relative min-h-0 overflow-hidden" data-merge-gap="right">
              <svg class="pointer-events-none absolute inset-0" :width="GAP_PX" :height="gapHeight" aria-hidden="true">
                <path v-for="s in rightShapes" :key="s.key" :d="s.d" :class="[`merge-curve-${s.kind}`, s.dismissed && 'merge-curve-dismissed']" />
              </svg>
              <template v-for="s in rightShapes" :key="`a${s.key}`">
                <template v-if="!s.dismissed">
                  <button
                    type="button"
                    class="merge-arrow merge-dismiss right-5"
                    :style="{ top: `${s.arrowY}px` }"
                    :title="`Leave these lines from ${names.theirs} out (undo brings them back)`"
                    data-merge-dismiss="right"
                    @click="dismiss('right', s.index)"
                  >
                    ×
                  </button>
                  <button
                    type="button"
                    class="merge-arrow right-0.5"
                    :style="{ top: `${s.arrowY}px` }"
                    :title="`Copy these lines from ${names.theirs} into the result (Ctrl+click to add them below)`"
                    data-merge-take="right"
                    @click="take('right', s.index, $event.ctrlKey || $event.metaKey)"
                  >
                    «
                  </button>
                </template>
              </template>
            </div>
            <div class="relative min-h-0 min-w-0">
              <div ref="rightHost" class="h-full overflow-hidden" data-merge-pane="right" />
              <!-- Marks for the pane's differences and conflicts, beside its scroll bar: click one to go there. -->
              <div class="merge-ruler" :style="{ right: `${rulers.right.right}px` }" aria-hidden="true" data-merge-ruler="right">
                <button
                  v-for="m in rulers.right.marks"
                  :key="m.key"
                  type="button"
                  tabindex="-1"
                  :class="`merge-ruler-${m.kind}`"
                  :style="{ top: `${m.top}%`, height: `max(2px, ${m.height}%)` }"
                  @click="scrollToLine('right', m.line)"
                />
              </div>
            </div>
          </div>
        </section>
      </div>

      <DialogFooter>
        <LoaderCircleIcon v-if="saving" class="mr-auto size-4 animate-spin text-muted-foreground" />
        <Button type="button" variant="outline" @click="onOpenChange(false)">Cancel</Button>
        <Button type="button" variant="outline" :disabled="!file || file.refused !== null" @click="save(false)">Save</Button>
        <span :title="resolveBlocked ?? ''">
          <Button type="button" variant="outline" :disabled="resolveBlocked !== null" @click="save(true, true)">Save, Resolve and Open Next</Button>
        </span>
        <span :title="resolveBlocked ?? ''">
          <Button type="button" :disabled="resolveBlocked !== null" @click="save(true)">Save and Mark Resolved</Button>
        </span>
      </DialogFooter>
    </DialogContent>
  </Dialog>
</template>
