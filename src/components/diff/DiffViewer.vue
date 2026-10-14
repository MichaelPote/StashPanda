<script setup lang="ts">
import { computed, ref, shallowRef, watch } from "vue";
import { useDebounceFn, useResizeObserver } from "@vueuse/core";
import { ChevronDownIcon, ChevronUpIcon, LoaderCircleIcon } from "@lucide/vue";
import { commands, type DiffText, type Eol, type LineAction, type ResolveSide, type SelectedLine } from "@/lib/bindings";
import { Button } from "@/components/ui/button";
import PanelHeader from "@/components/shell/PanelHeader.vue";
import EmptyState from "@/components/shell/EmptyState.vue";
import { useVirtualRows } from "@/composables/useVirtualRows";
import { diffRowPx, diffTextStyle, displayLength, resolveDiffFont } from "@/lib/diff/font";
import { highlightPair, parseUnified, type DiffLine, type Segment } from "@/lib/diff/parseUnified";
import { errorMessage, IpcError, unwrap } from "@/lib/ipc";
import { conflictComparisons, conflictStages, conflictSummary, operationNames, type ConflictComparison } from "@/lib/conflicts";
import type { FileItem } from "@/lib/files";
import { markResolved, openFileReason, openWorktreeFile, opFailureText, runOp, takeVersion } from "@/lib/repoActions";
import { cn } from "@/lib/utils";
import { useDialogsStore } from "@/stores/dialogs";
import { useSessionsStore, type RepoSession } from "@/stores/sessions";
import { useSettingsStore } from "@/stores/settings";
import { useUiStateStore } from "@/stores/uiState";

const props = defineProps<{
  session: RepoSession;
}>();

const settings = useSettingsStore();
const uiState = useUiStateStore();
const sessions = useSessionsStore();
const dialogs = useDialogsStore();

/** Font, size, line height, tab width and ligatures from the `diff` section of settings.json. */
const font = computed(() => resolveDiffFont(settings.settings?.diff));
const textStyle = computed(() => diffTextStyle(font.value));
const wordHighlight = computed(() => settings.settings?.diff?.wordHighlight ?? true);
/**
 * Row height in whole pixels: the font size times the line height. The virtual list and the rows
 * both use this one number, so rows never drift from where the list expects them.
 */
const rowPx = computed(() => diffRowPx(font.value));
const ALL_CONTEXT = 99999;

type Mode = "unified" | "side";
// The header's choices and the divider position, remembered in the UI state.
const mode = uiState.pref<Mode>((s) => s.diff?.layout, "unified", (layout) => ({ diff: { layout } }));
const contextLines = uiState.pref<number>((s) => s.diff?.contextLines, 3, (contextLines) => ({ diff: { contextLines } }));
const ignoreWhitespace = uiState.pref<boolean>((s) => s.diff?.ignoreWhitespace, false, (ignoreWhitespace) => ({ diff: { ignoreWhitespace } }));
const ignoreCrAtEol = uiState.pref<boolean>((s) => s.diff?.ignoreCrAtEol, false, (ignoreCrAtEol) => ({ diff: { ignoreCrAtEol } }));
/** Share of the side-by-side width given to the old (left) side. */
const split = uiState.pref<number>((s) => s.diff?.split, 0.5, (split) => ({ diff: { split } }));

const CONTEXT_CHOICES: { value: number; label: string }[] = [
  { value: 3, label: "3 lines" },
  { value: 10, label: "10 lines" },
  { value: ALL_CONTEXT, label: "All" },
];

// ---- Loading -------------------------------------------------------------------------------

const diff = shallowRef<DiffText | null>(null);
const loading = ref(false);
const error = ref<string | null>(null);
let request = 0;

async function load(): Promise<void> {
  const file = props.session.selectedFile;
  const id = (request += 1);
  if (!file) {
    diff.value = null;
    error.value = null;
    loading.value = false;
    return;
  }
  loading.value = true;
  error.value = null;
  try {
    const result = await unwrap(
      commands.getFileDiff(props.session.key, file.target, file.path, file.oldPath ?? null, {
        context: contextLines.value,
        ignoreWhitespace: ignoreWhitespace.value,
        ignoreCrAtEol: ignoreCrAtEol.value,
      }),
    );
    if (id !== request) return;
    diff.value = result;
    scroller.value?.scrollTo({ top: 0 });
    resetSideScroll();
  } catch (e) {
    if (id !== request) return;
    diff.value = null;
    error.value = errorMessage(e);
  } finally {
    if (id === request) loading.value = false;
  }
}

const loadDebounced = useDebounceFn(load, 50);

watch(
  () => [props.session.selectedFile, contextLines.value, ignoreWhitespace.value, ignoreCrAtEol.value] as const,
  () => {
    void loadDebounced();
  },
  { immediate: true },
);

// ---- Parsing and pairing -------------------------------------------------------------------

const parsed = computed(() => (diff.value && !diff.value.isBinary ? parseUnified(diff.value.text) : null));
const file = computed(() => parsed.value?.files[0] ?? null);

interface Pair {
  old: string;
  new: string;
}

interface UnifiedRow {
  kind: "hunk" | "line";
  hunk: number;
  header?: string;
  line?: DiffLine;
  pair?: number;
}

interface SideRow {
  kind: "hunk" | "pair" | "meta";
  hunk: number;
  header?: string;
  left?: DiffLine;
  right?: DiffLine;
  pair?: number;
  text?: string;
}

interface Built {
  unified: UnifiedRow[];
  side: SideRow[];
  pairs: Pair[];
  maxLen: number;
  /** Longest line on each side of the side-by-side view, in characters with tabs expanded. */
  leftMax: number;
  rightMax: number;
}

const built = computed<Built>(() => {
  const unified: UnifiedRow[] = [];
  const side: SideRow[] = [];
  const pairs: Pair[] = [];
  let maxLen = 0;
  let leftMax = 0;
  let rightMax = 0;
  const f = file.value;
  if (!f) return { unified, side, pairs, maxLen, leftMax, rightMax };
  const tab = font.value.tabWidth;

  f.hunks.forEach((hunk, h) => {
    unified.push({ kind: "hunk", hunk: h, header: hunk.header });
    side.push({ kind: "hunk", hunk: h, header: hunk.header });
    maxLen = Math.max(maxLen, hunk.header.length);

    let dels: DiffLine[] = [];
    let adds: DiffLine[] = [];
    // `\ No newline at end of file` lines sit between a del run and its add run; they must not
    // break the pairing, so they are held back and emitted after the run.
    let metas: DiffLine[] = [];
    const flush = (): void => {
      const n = Math.max(dels.length, adds.length);
      const pairIds: (number | undefined)[] = [];
      for (let i = 0; i < n; i += 1) {
        const d = dels[i];
        const a = adds[i];
        let pair: number | undefined;
        if (d && a) {
          pair = pairs.length;
          pairs.push({ old: d.text, new: a.text });
        }
        pairIds.push(pair);
        if (d) leftMax = Math.max(leftMax, displayLength(d.text, tab));
        if (a) rightMax = Math.max(rightMax, displayLength(a.text, tab));
        side.push({ kind: "pair", hunk: h, left: d, right: a, pair });
      }
      dels.forEach((d, i) => unified.push({ kind: "line", hunk: h, line: d, pair: pairIds[i] }));
      adds.forEach((a, i) => unified.push({ kind: "line", hunk: h, line: a, pair: pairIds[i] }));
      for (const m of metas) {
        unified.push({ kind: "line", hunk: h, line: m });
        side.push({ kind: "meta", hunk: h, text: m.text });
      }
      dels = [];
      adds = [];
      metas = [];
    };

    for (const line of hunk.lines) {
      maxLen = Math.max(maxLen, displayLength(line.text, tab));
      if (line.kind === "del") {
        dels.push(line);
        continue;
      }
      if (line.kind === "add") {
        adds.push(line);
        continue;
      }
      if (line.kind === "meta") {
        metas.push(line);
        continue;
      }
      flush();
      unified.push({ kind: "line", hunk: h, line });
      const len = displayLength(line.text, tab);
      leftMax = Math.max(leftMax, len);
      rightMax = Math.max(rightMax, len);
      side.push({ kind: "pair", hunk: h, left: line, right: line });
    }
    flush();
  });
  return { unified, side, pairs, maxLen, leftMax, rightMax };
});

const rowCount = computed(() => (mode.value === "unified" ? built.value.unified.length : built.value.side.length));

const segmentCache = new Map<number, { old: Segment[]; new: Segment[] }>();
watch(built, () => segmentCache.clear());

function segments(pair: number | undefined, sideOf: "old" | "new", fallback: string): Segment[] {
  if (pair === undefined || !wordHighlight.value) return [{ text: fallback, changed: false }];
  let entry = segmentCache.get(pair);
  if (!entry) {
    const p = built.value.pairs[pair];
    if (!p) return [{ text: fallback, changed: false }];
    entry = highlightPair(p.old, p.new);
    segmentCache.set(pair, entry);
  }
  return sideOf === "old" ? entry.old : entry.new;
}

// ---- Virtualisation ------------------------------------------------------------------------

const scroller = ref<HTMLDivElement | null>(null);
const virtual = useVirtualRows(scroller, { rowHeight: () => rowPx.value, total: () => rowCount.value, overscan: 10 });

const visibleUnified = computed(() => built.value.unified.slice(virtual.start.value, virtual.end.value));
const visibleSide = computed(() => built.value.side.slice(virtual.start.value, virtual.end.value));

/**
 * Unified only: the rows are as wide as the longest line and the whole view scrolls sideways. The
 * gutter is the border, two line numbers and the marker (widths from `diffTextStyle`).
 */
const contentMinWidth = computed(
  () => `calc(2px + 2 * var(--diff-num-w) + var(--diff-mark-w) + ${Math.max(20, built.value.maxLen + 2)}ch)`,
);

// ---- Side by side: split, divider and horizontal scrolling --------------------------------
//
// The two sides share the vertical scroll, so their rows stay level, but each clips its own text
// and scrolls it sideways with its own scrollbar under it. Moving one scrollbar moves the other
// to the same fraction of its own range. The line numbers do not move sideways.

const SPLIT_MIN = 0.15;
const SPLIT_MAX = 0.85;
/** Width of the rows (the scroller without its vertical scrollbar) and of that scrollbar. */
const rowsWidth = ref(0);
const scrollbarPx = ref(0);
useResizeObserver(scroller, () => {
  const el = scroller.value;
  if (!el) return;
  rowsWidth.value = el.clientWidth;
  scrollbarPx.value = el.offsetWidth - el.clientWidth;
});

const leftPx = computed(() => Math.round(Math.min(SPLIT_MAX, Math.max(SPLIT_MIN, split.value)) * rowsWidth.value));
const rightPx = computed(() => Math.max(0, rowsWidth.value - leftPx.value));
const sideStyle = computed(() => ({ "--left-w": `${leftPx.value}px`, "--right-w": `${rightPx.value}px` }));

const splitDragging = ref(false);

function startSplitDrag(event: PointerEvent): void {
  if (event.button !== 0 || rowsWidth.value <= 0) return;
  const target = event.currentTarget as HTMLElement;
  target.setPointerCapture(event.pointerId);
  const startX = event.clientX;
  const startLeft = leftPx.value;
  splitDragging.value = true;
  const onMove = (e: PointerEvent): void => {
    const next = (startLeft + e.clientX - startX) / rowsWidth.value;
    split.value = Math.min(SPLIT_MAX, Math.max(SPLIT_MIN, next));
  };
  const onUp = (): void => {
    splitDragging.value = false;
    target.removeEventListener("pointermove", onMove);
    target.removeEventListener("pointerup", onUp);
    target.removeEventListener("pointercancel", onUp);
  };
  target.addEventListener("pointermove", onMove);
  target.addEventListener("pointerup", onUp);
  target.addEventListener("pointercancel", onUp);
  event.preventDefault();
}

type Side = "left" | "right";
const stripLeft = ref<HTMLDivElement | null>(null);
const stripRight = ref<HTMLDivElement | null>(null);
/** The scroll position this code just gave a strip, so its own scroll event is not synced back. */
const expected: Record<Side, number | null> = { left: null, right: null };

function strip(side: Side): HTMLDivElement | null {
  return side === "left" ? stripLeft.value : stripRight.value;
}

/** Moves one side's text; set as a CSS variable so no row has to re-render while scrolling. */
function applyOffset(side: Side, px: number): void {
  scroller.value?.style.setProperty(side === "left" ? "--sx-left" : "--sx-right", `${px}px`);
}

function onStripScroll(side: Side): void {
  const src = strip(side);
  if (!src) return;
  applyOffset(side, src.scrollLeft);
  const wanted = expected[side];
  expected[side] = null;
  if (wanted !== null && Math.abs(src.scrollLeft - wanted) <= 1) return;
  const other: Side = side === "left" ? "right" : "left";
  const dst = strip(other);
  if (!dst) return;
  const srcMax = src.scrollWidth - src.clientWidth;
  const dstMax = dst.scrollWidth - dst.clientWidth;
  const target = Math.round((srcMax > 0 ? src.scrollLeft / srcMax : 0) * Math.max(0, dstMax));
  if (Math.abs(dst.scrollLeft - target) > 1) {
    expected[other] = target;
    dst.scrollLeft = target;
  }
}

/** A sideways wheel or trackpad movement over the rows scrolls the side under the pointer. */
function onSideWheel(event: WheelEvent): void {
  const dx = event.deltaX !== 0 ? event.deltaX : event.shiftKey ? event.deltaY : 0;
  const el = scroller.value;
  if (dx === 0 || !el) return;
  const x = event.clientX - el.getBoundingClientRect().left;
  const target = strip(x < leftPx.value ? "left" : "right");
  if (!target) return;
  event.preventDefault();
  target.scrollLeft += dx;
}

function resetSideScroll(): void {
  for (const side of ["left", "right"] as const) {
    const el = strip(side);
    if (el) el.scrollLeft = 0;
    expected[side] = null;
    applyOffset(side, 0);
  }
}

watch(mode, () => resetSideScroll());

// ---- Hunk navigation -----------------------------------------------------------------------

const hunkRows = computed<number[]>(() => {
  const rows = mode.value === "unified" ? built.value.unified : built.value.side;
  const out: number[] = [];
  rows.forEach((r, i) => {
    if (r.kind === "hunk") out.push(i);
  });
  return out;
});

function firstVisibleRow(): number {
  return Math.floor(virtual.scrollTop.value / rowPx.value);
}

function nextHunk(): void {
  const cur = firstVisibleRow();
  const target = hunkRows.value.find((i) => i > cur + 1);
  if (target !== undefined) virtual.scrollTo(target * rowPx.value);
}

function prevHunk(): void {
  const cur = firstVisibleRow();
  const before = hunkRows.value.filter((i) => i < cur - 1);
  const target = before[before.length - 1];
  virtual.scrollTo((target ?? 0) * rowPx.value);
}

// ---- Presentation --------------------------------------------------------------------------

const selected = computed(() => props.session.selectedFile);

const EOL_NAMES: Record<Eol, string | null> = { lf: "LF", crLf: "CRLF", mixed: "Mixed", none: null };
/**
 * The line endings of the lines the diff shows: one name when both versions agree ("CRLF"), the
 * change when they differ ("LF → CRLF"), or null when the diff shows no line ends.
 */
const eolText = computed(() => {
  const d = diff.value;
  if (!d || d.isBinary) return null;
  const [before, after] = [EOL_NAMES[d.oldEol], EOL_NAMES[d.newEol]];
  if (before && after && before !== after) return `${before} → ${after}`;
  return after ?? before;
});
const eolChanged = computed(() => eolText.value?.includes("→") ?? false);
const headerPath = computed(() => {
  const s = selected.value;
  if (!s) return "";
  return s.oldPath && s.oldPath !== s.path ? `${s.oldPath} → ${s.path}` : s.path;
});

/** The class a `diff-line` row adds for its kind (`assets/diff.css`); context lines add none. */
function lineClass(kind: DiffLine["kind"] | undefined): string {
  switch (kind) {
    case "add":
      return "diff-line-add";
    case "del":
      return "diff-line-del";
    case "meta":
      return "diff-line-meta";
    default:
      return "";
  }
}

function marker(kind: DiffLine["kind"] | undefined): string {
  switch (kind) {
    case "add":
      return "+";
    case "del":
      return "-";
    case "meta":
      return "\\";
    default:
      return " ";
  }
}

// ---- Staging, unstaging and discarding lines and hunks --------------------------------------
//
// Only for the working tree's diffs: an unstaged or untracked file's lines can be staged or
// discarded, a staged file's lines unstaged. Clicking a changed line's number picks it (Shift-click
// picks the range from the last one); each hunk header has buttons for its whole hunk. The backend
// recomputes the diff itself and checks every picked line against it, so the context and
// whitespace settings here do not matter.

/** What can be done to this diff's lines: stage (and discard) them, unstage them, or nothing. */
const lineMode = computed<"stage" | "unstage" | null>(() => {
  switch (selected.value?.target.type) {
    case "unstaged":
    case "untracked":
      return "stage";
    case "staged":
      return "unstage";
    default:
      return null;
  }
});

/** The picked lines, as `a<new line number>` for an addition and `d<old line number>` for a removal. */
const picked = shallowRef<Set<string>>(new Set());
let lastPicked: string | null = null;
const busy = ref(false);

function pickKey(line: DiffLine | undefined): string | null {
  if (line?.kind === "add" && line.newNo !== undefined) return `a${line.newNo}`;
  if (line?.kind === "del" && line.oldNo !== undefined) return `d${line.oldNo}`;
  return null;
}

/** Every changed line in diff order, for Shift-click ranges and for finding picked lines. */
const changeLines = computed<DiffLine[]>(() => (file.value?.hunks ?? []).flatMap((h) => h.lines.filter((l) => l.kind === "add" || l.kind === "del")));

watch(diff, () => {
  picked.value = new Set();
  lastPicked = null;
});

function isPicked(line: DiffLine | undefined): boolean {
  const key = pickKey(line);
  return key !== null && picked.value.has(key);
}

function canPick(line: DiffLine | undefined): boolean {
  return lineMode.value !== null && pickKey(line) !== null && !busy.value;
}

function onPick(line: DiffLine | undefined, event: MouseEvent): void {
  const key = pickKey(line);
  if (event.button !== 0 || key === null || !canPick(line)) return;
  const next = new Set(picked.value);
  const keys = changeLines.value.map((l) => pickKey(l));
  if (event.shiftKey && lastPicked !== null && keys.includes(lastPicked)) {
    const [a, b] = [keys.indexOf(lastPicked), keys.indexOf(key)].sort((x, y) => x - y);
    for (const k of keys.slice(a, b + 1)) if (k !== null) next.add(k);
  } else if (next.has(key)) {
    next.delete(key);
  } else {
    next.add(key);
  }
  lastPicked = key;
  picked.value = next;
  // A click on a number would otherwise start selecting the line's text.
  event.preventDefault();
}

const pickedLines = computed(() => changeLines.value.filter((l) => picked.value.has(pickKey(l) ?? "")));

function hunkLines(hunk: number): DiffLine[] {
  return (file.value?.hunks[hunk]?.lines ?? []).filter((l) => l.kind === "add" || l.kind === "del");
}

function linesLabel(n: number): string {
  return n === 1 ? "1 line" : `${n} lines`;
}

/** Stages, unstages or discards `lines` of the shown file, then reloads the diff. */
async function applyLines(action: LineAction, lines: DiffLine[], what: string): Promise<void> {
  const f = selected.value;
  if (!f || lines.length === 0 || busy.value) return;
  const key = props.session.key;
  if (action === "discard") {
    const confirmed = await dialogs.open("confirm", {
      title: `Discard ${what}?`,
      message: `${f.path}\n\nThe ${what === "this hunk" ? "hunk's changes go" : "picked changes go"} back to the staged version of the file, or to the last commit when nothing is staged. This cannot be undone.`,
      confirmLabel: "Discard",
      destructive: true,
    });
    if (!confirmed) return;
  }
  const chosen: SelectedLine[] = lines.map((l) => ({
    kind: l.kind === "add" ? "add" : "del",
    line: (l.kind === "add" ? l.newNo : l.oldNo) ?? 0,
    text: l.text,
  }));
  const [done, doing] = action === "stage" ? ["Staged", "Staging"] : action === "unstage" ? ["Unstaged", "Unstaging"] : ["Discarded", "Discarding"];
  busy.value = true;
  try {
    const result = await runOp(key, () => commands.opApplyLines(key, action, f.path, f.target.type === "untracked", chosen));
    if (result.ok) {
      sessions.notifySuccess(`${done} ${what} of ${f.path}.`);
      picked.value = new Set();
    } else {
      sessions.notify(opFailureText(result, `${doing} ${what} of ${f.path}`));
    }
  } catch (e) {
    // A refused selection explains itself; the generic wording ("The path … is not valid") would not.
    if (e instanceof IpcError && e.error.kind === "invalidPath") sessions.notify(e.error.reason.charAt(0).toUpperCase() + e.error.reason.slice(1) + ".");
    else sessions.notify(errorMessage(e));
  } finally {
    busy.value = false;
    await load();
  }
}

// ---- Conflicted files ----------------------------------------------------------------------
//
// A conflicted file has up to three versions in the index (the base both sides started from, and
// each side's) besides the file on disk. The strip picks which two to compare, and resolves the
// file: keep one side's version, or mark the file on disk as resolved.

const conflictFile = computed(() => {
  const s = selected.value;
  if (s?.target.type !== "conflict") return null;
  const entry = props.session.status?.entries.find((e) => e.conflicted && e.path === s.path);
  const letters = entry ? `${entry.x}${entry.y}` : "UU";
  const item: FileItem = { id: `conflicted:${s.path}`, path: s.path, oldPath: null, letter: letters, state: "conflict", added: null, deleted: null, binary: false, target: s.target };
  return { item, stages: conflictStages(letters), letters };
});
const sideNames = computed(() => operationNames(props.session.status, props.session.refs));
const comparisons = computed(() => conflictComparisons(sideNames.value));
const openReason = computed(() => openFileReason(props.session));

function isComparing(c: ConflictComparison): boolean {
  const t = selected.value?.target;
  return t?.type === "conflict" && t.from === c.from && t.to === c.to;
}

/** Why a comparison has nothing on one side for this file, or null. */
function comparisonNote(c: ConflictComparison): string | null {
  const stages = conflictFile.value?.stages;
  if (!stages) return null;
  const missing = [c.from, c.to].find((v) => v !== "workingTree" && !stages[v]);
  if (missing === "base") return "Both sides added this file, so there is no base version: the other side shows as wholly added.";
  if (missing) return `${missing === "ours" ? sideNames.value.ours : sideNames.value.theirs} deleted this file, so that side is empty.`;
  return null;
}

function compare(c: ConflictComparison): void {
  const s = selected.value;
  if (!s) return;
  props.session.conflictComparison = { from: c.from, to: c.to };
  props.session.selectedFile = { ...s, target: { type: "conflict", from: c.from, to: c.to } };
}

function takeTitle(side: ResolveSide): string {
  const f = conflictFile.value;
  const name = side === "ours" ? sideNames.value.ours : sideNames.value.theirs;
  if (f && !f.stages[side]) return `${name} deleted this file, so taking its version deletes the file and marks the conflict resolved.`;
  return `Replace the file with ${name}'s version, throwing away the other side's changes and any edits to the file, and mark the conflict resolved.`;
}

function toggleContext(value: string): void {
  const n = Number(value);
  if (Number.isFinite(n) && n > 0) contextLines.value = n;
}
</script>

<template>
  <section class="area flex h-full min-h-0 flex-col">
    <PanelHeader title="Diff">
      <!-- The same look as the path beside the Commits header (CommitGraphView.vue), and selectable like it. -->
      <span v-if="selected" class="truncate text-xs text-muted-foreground select-text" :title="headerPath">{{ headerPath }}</span>
      <span
        v-if="eolText"
        :class="cn('shrink-0 rounded-sm border px-1 font-mono text-[0.65rem] leading-[14px]', eolChanged ? 'border-modified/60 text-modified-fg' : 'border-border-soft text-muted-foreground')"
        :title="eolChanged ? 'The line endings change in this diff' : 'Line endings of the lines this diff shows'"
      >
        {{ eolText }}
      </span>
      <LoaderCircleIcon v-if="loading" class="size-3 shrink-0 animate-spin text-muted-foreground" />
      <template #actions>
        <div v-if="selected" class="flex items-center gap-1 text-[0.7rem]">
          <div class="inline-flex overflow-hidden rounded-md border border-border-soft">
            <button
              type="button"
              :aria-pressed="mode === 'unified'"
              class="toggle h-[18px] px-2 leading-none font-medium rounded-none border-0"
              @click="mode = 'unified'"
            >
              Unified
            </button>
            <button
              type="button"
              :aria-pressed="mode === 'side'"
              class="toggle h-[18px] px-2 leading-none font-medium rounded-none border-0 border-l border-l-border-soft"
              @click="mode = 'side'"
            >
              Side by side
            </button>
          </div>
          <label class="inline-flex items-center gap-1 text-muted-foreground">
            Context
            <select
              class="well h-[17px] rounded-md border px-1 text-[0.7rem] text-foreground"
              :value="String(contextLines)"
              @change="toggleContext(($event.target as HTMLSelectElement).value)"
            >
              <option v-for="c in CONTEXT_CHOICES" :key="c.value" :value="String(c.value)">{{ c.label }}</option>
            </select>
          </label>
          <button
            type="button"
            :aria-pressed="ignoreWhitespace"
            class="toggle h-[18px] px-2 leading-none font-medium rounded-sm border"
            title="Ignore whitespace changes (git diff -w)"
            @click="ignoreWhitespace = !ignoreWhitespace"
          >
            Ignore whitespace
          </button>
          <button
            type="button"
            :aria-pressed="ignoreCrAtEol"
            class="toggle h-[18px] px-2 leading-none font-medium rounded-sm border"
            title="Ignore carriage returns at the end of lines (git diff --ignore-cr-at-eol)"
            @click="ignoreCrAtEol = !ignoreCrAtEol"
          >
            Ignore CR at EOL
          </button>
          <span class="ml-1 text-muted-foreground tabular-nums">{{ hunkRows.length }} hunks</span>
          <Button variant="ghost" size="icon-xs" class="size-5" title="Previous hunk" :disabled="hunkRows.length === 0" @click="prevHunk">
            <ChevronUpIcon />
          </Button>
          <Button variant="ghost" size="icon-xs" class="size-5" title="Next hunk" :disabled="hunkRows.length === 0" @click="nextHunk">
            <ChevronDownIcon />
          </Button>
        </div>
      </template>
    </PanelHeader>

    <!-- A conflicted file: what each side did, which versions to compare, and how to resolve it. -->
    <div v-if="conflictFile" class="flex shrink-0 flex-wrap items-center gap-x-1.5 gap-y-1 border-b border-border-soft band-conflict px-2 py-1 text-[0.7rem]" data-conflict-strip>
      <span class="text-foreground">{{ conflictSummary(conflictFile.letters, sideNames) }}</span>
      <span class="ml-1 text-muted-foreground">Compare</span>
      <div class="inline-flex overflow-hidden rounded-md border border-border-soft">
        <button
          v-for="(c, i) in comparisons"
          :key="`${c.from}-${c.to}`"
          type="button"
          :aria-pressed="isComparing(c)"
          :class="cn('toggle h-[18px] rounded-none border-0 px-2 leading-none font-medium', i > 0 && 'border-l border-l-border-soft')"
          :title="comparisonNote(c) ? `${c.title}. ${comparisonNote(c)}` : c.title"
          :data-compare="`${c.from}-${c.to}`"
          @click="compare(c)"
        >
          {{ c.label }}
        </button>
      </div>
      <span class="ml-auto flex flex-wrap items-center gap-1">
        <button type="button" class="diff-action" :title="takeTitle('ours')" data-resolve="ours" @click="takeVersion(session, [conflictFile.item], 'ours')">
          Take the version from {{ sideNames.ours }}
        </button>
        <button type="button" class="diff-action" :title="takeTitle('theirs')" data-resolve="theirs" @click="takeVersion(session, [conflictFile.item], 'theirs')">
          Take the version from {{ sideNames.theirs }}
        </button>
        <button
          type="button"
          class="diff-action"
          title="After editing the file to resolve the conflict, stage it as it is on disk (git add)"
          data-resolve="mark"
          @click="markResolved(session, [conflictFile.item])"
        >
          Mark as resolved
        </button>
        <button
          type="button"
          :class="cn('diff-action', openReason && 'opacity-50')"
          :title="openReason ?? 'Open the file in the program your system uses for it, to edit it by hand'"
          :aria-disabled="openReason !== null"
          @click="!openReason && openWorktreeFile(session.key, conflictFile.item.path)"
        >
          Open
        </button>
      </span>
    </div>

    <EmptyState v-if="!selected" message="Select a file to see its changes." />
    <EmptyState v-else-if="error" :message="error" class="text-destructive" />
    <EmptyState v-else-if="diff && diff.isBinary" message="Binary file" />
    <EmptyState v-else-if="diff && rowCount === 0 && !loading" message="No differences to show." />

    <template v-else>
      <div
        v-if="diff && diff.truncated"
        class="shrink-0 border-b border-modified/40 bg-modified/10 px-2 py-0.5 text-[0.7rem] text-modified-fg"
        role="status"
      >
        This diff was truncated by the backend ({{ diff.bytes.toLocaleString() }} bytes); the rows below are incomplete.
      </div>

      <!-- Picking lines: what is picked, and what can be done with it. -->
      <div v-if="lineMode && changeLines.length > 0" class="flex h-[22px] shrink-0 items-center gap-1.5 border-b border-border-soft bg-muted px-2 text-[0.7rem] text-muted-foreground">
        <template v-if="pickedLines.length === 0">Click the line numbers of changed lines to pick them; Shift-click picks a range.</template>
        <template v-else>
          <span class="text-foreground tabular-nums">{{ linesLabel(pickedLines.length) }} picked</span>
          <template v-if="lineMode === 'stage'">
            <button type="button" class="diff-action" :disabled="busy" @click="applyLines('stage', pickedLines, linesLabel(pickedLines.length))">Stage</button>
            <button type="button" class="diff-action diff-action-destructive" :disabled="busy" @click="applyLines('discard', pickedLines, linesLabel(pickedLines.length))">Discard…</button>
          </template>
          <button v-else type="button" class="diff-action" :disabled="busy" @click="applyLines('unstage', pickedLines, linesLabel(pickedLines.length))">Unstage</button>
          <button type="button" class="diff-action" :disabled="busy" @click="picked = new Set()">Clear</button>
        </template>
        <LoaderCircleIcon v-if="busy" class="size-3 animate-spin" />
      </div>

      <div class="relative flex min-h-0 flex-1 flex-col" :style="mode === 'side' ? sideStyle : undefined">
      <div
        ref="scroller"
        :class="cn('relative min-h-0 flex-1 select-text scrollbar-thin', mode === 'side' ? 'overflow-x-hidden overflow-y-auto' : 'overflow-auto')"
        :style="textStyle"
        @wheel="mode === 'side' ? onSideWheel($event) : undefined"
      >
        <div class="relative" :style="mode === 'side' ? { height: `${rowCount * rowPx}px` } : { height: `${rowCount * rowPx}px`, minWidth: contentMinWidth }">
          <!-- No will-change-transform here: it would put the rows on a compositor layer of their own,
               where Chromium never draws ClearType, so the text would come out grayscale and soft
               whatever diff.fontSmoothing says. See smoothingStyle in lib/diff/font.ts. -->
          <div class="absolute inset-x-0 top-0" :style="{ transform: `translateY(${virtual.start.value * rowPx}px)` }">
            <!-- Unified -->
            <template v-if="mode === 'unified'">
              <template v-for="(row, k) in visibleUnified" :key="virtual.start.value + k">
                <div v-if="row.kind === 'hunk'" class="diff-hunk">
                  <span v-if="lineMode" class="diff-hunk-actions">
                    <template v-if="lineMode === 'stage'">
                      <button type="button" class="diff-action" :disabled="busy" @click="applyLines('stage', hunkLines(row.hunk), 'this hunk')">Stage hunk</button>
                      <button type="button" class="diff-action diff-action-destructive" :disabled="busy" @click="applyLines('discard', hunkLines(row.hunk), 'this hunk')">Discard…</button>
                    </template>
                    <button v-else type="button" class="diff-action" :disabled="busy" @click="applyLines('unstage', hunkLines(row.hunk), 'this hunk')">Unstage hunk</button>
                  </span>
                  <span class="diff-glyphs"> {{ row.header }}</span>
                </div>
                <div v-else :class="['diff-line', lineClass(row.line?.kind), isPicked(row.line) && 'diff-line-picked']">
                  <span :class="['diff-num', canPick(row.line) && 'diff-num-pick']" @mousedown="onPick(row.line, $event)"><span class="diff-glyphs">{{ row.line?.oldNo ?? "" }}</span></span>
                  <span :class="['diff-num', canPick(row.line) && 'diff-num-pick']" @mousedown="onPick(row.line, $event)"><span class="diff-glyphs">{{ row.line?.newNo ?? "" }}</span></span>
                  <span class="diff-sign">{{ marker(row.line?.kind) }}</span>
                  <span class="diff-code">
                    <template v-for="(seg, i) in segments(row.pair, row.line?.kind === 'del' ? 'old' : 'new', row.line?.text ?? '')" :key="i">
                      <mark v-if="seg.changed" :class="row.line?.kind === 'del' ? 'diff-word-del' : 'diff-word-add'"><span class="diff-glyphs">{{ seg.text }}</span></mark>
                      <span v-else class="diff-glyphs">{{ seg.text }}</span>
                    </template>
                  </span>
                </div>
              </template>
            </template>

            <!-- Side by side -->
            <template v-else>
              <template v-for="(row, k) in visibleSide" :key="virtual.start.value + k">
                <div v-if="row.kind === 'hunk'" class="diff-hunk overflow-hidden">
                  <span v-if="lineMode" class="diff-hunk-actions">
                    <template v-if="lineMode === 'stage'">
                      <button type="button" class="diff-action" :disabled="busy" @click="applyLines('stage', hunkLines(row.hunk), 'this hunk')">Stage hunk</button>
                      <button type="button" class="diff-action diff-action-destructive" :disabled="busy" @click="applyLines('discard', hunkLines(row.hunk), 'this hunk')">Discard…</button>
                    </template>
                    <button v-else type="button" class="diff-action" :disabled="busy" @click="applyLines('unstage', hunkLines(row.hunk), 'this hunk')">Unstage hunk</button>
                  </span>
                  <span class="diff-glyphs"> {{ row.header }}</span>
                </div>
                <div v-else-if="row.kind === 'meta'" class="diff-side-meta">
                  <span class="diff-glyphs"> \ {{ row.text }}</span>
                </div>
                <div v-else class="diff-side-row h-(--diff-row-h)">
                  <div :class="['diff-line min-w-0 overflow-hidden', row.left ? lineClass(row.left.kind === 'context' ? 'context' : 'del') : 'diff-line-empty', row.left?.kind === 'del' && isPicked(row.left) && 'diff-line-picked']">
                    <span :class="['diff-num', row.left?.kind === 'del' && canPick(row.left) && 'diff-num-pick']" @mousedown="row.left?.kind === 'del' && onPick(row.left, $event)"><span class="diff-glyphs">{{ row.left?.oldNo ?? "" }}</span></span>
                    <span class="diff-sign">{{ row.left ? marker(row.left.kind === 'context' ? 'context' : 'del') : "" }}</span>
                    <span class="diff-side-code diff-text-left"><span>
                      <template v-if="row.left">
                        <template v-for="(seg, i) in segments(row.pair, 'old', row.left.text)" :key="i">
                          <mark v-if="seg.changed" class="diff-word-del"><span class="diff-glyphs">{{ seg.text }}</span></mark>
                          <span v-else class="diff-glyphs">{{ seg.text }}</span>
                        </template>
                      </template>
                    </span></span>
                  </div>
                  <div :class="['diff-line min-w-0 overflow-hidden', row.right ? lineClass(row.right.kind === 'context' ? 'context' : 'add') : 'diff-line-empty', row.right?.kind === 'add' && isPicked(row.right) && 'diff-line-picked']">
                    <span :class="['diff-num', row.right?.kind === 'add' && canPick(row.right) && 'diff-num-pick']" @mousedown="row.right?.kind === 'add' && onPick(row.right, $event)"><span class="diff-glyphs">{{ row.right?.newNo ?? "" }}</span></span>
                    <span class="diff-sign">{{ row.right ? marker(row.right.kind === 'context' ? 'context' : 'add') : "" }}</span>
                    <span class="diff-side-code diff-text-right"><span>
                      <template v-if="row.right">
                        <template v-for="(seg, i) in segments(row.pair, 'new', row.right.text)" :key="i">
                          <mark v-if="seg.changed" class="diff-word-add"><span class="diff-glyphs">{{ seg.text }}</span></mark>
                          <span v-else class="diff-glyphs">{{ seg.text }}</span>
                        </template>
                      </template>
                    </span></span>
                  </div>
                </div>
              </template>
            </template>
          </div>
        </div>
      </div>

      <!-- Side by side: one horizontal scrollbar under each side's text, level with the rows above. -->
      <div v-if="mode === 'side'" class="flex shrink-0" :style="[textStyle, { paddingRight: `${scrollbarPx}px`, lineHeight: 'normal' }]">
        <div class="flex" :style="{ width: `${leftPx}px` }">
          <div class="diff-gutter-space shrink-0" />
          <div ref="stripLeft" class="min-w-0 flex-1 overflow-x-auto overflow-y-hidden scrollbar-thin" @scroll="onStripScroll('left')">
            <div class="h-px" :style="{ width: `calc(${built.leftMax}ch + 0.5rem)` }" />
          </div>
        </div>
        <div class="flex" :style="{ width: `${rightPx}px` }">
          <div class="diff-gutter-space shrink-0" />
          <div ref="stripRight" class="min-w-0 flex-1 overflow-x-auto overflow-y-hidden scrollbar-thin" @scroll="onStripScroll('right')">
            <div class="h-px" :style="{ width: `calc(${built.rightMax}ch + 0.5rem)` }" />
          </div>
        </div>
      </div>

      <!-- Divider between the two sides: drag to move it, double-click to go back to half and half. -->
      <button
        v-if="mode === 'side' && rowsWidth > 0"
        type="button"
        class="group absolute top-0 bottom-0 z-10 flex w-[7px] cursor-col-resize justify-center outline-none"
        :style="{ left: `${leftPx - 3}px` }"
        aria-label="Resize the two sides of the diff"
        title="Drag to resize the two sides; double-click to split them evenly"
        @pointerdown="startSplitDrag"
        @dblclick="split = 0.5"
      >
        <span :class="['h-full w-px transition-colors', splitDragging ? 'w-[2px] bg-primary' : 'bg-border group-hover:w-[2px] group-hover:bg-primary/70']" />
      </button>
      </div>
    </template>
  </section>
</template>

