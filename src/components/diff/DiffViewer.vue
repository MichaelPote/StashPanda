<script setup lang="ts">
import { computed, nextTick, ref, shallowRef, useId, watch } from "vue";
import { useDebounceFn, usePreferredDark, useResizeObserver } from "@vueuse/core";
import { ChevronDownIcon, ChevronUpIcon, HistoryIcon, LoaderCircleIcon, ScanTextIcon } from "@lucide/vue";
import { commands, type DiffText, type Eol, type LineAction, type ResolveSide, type SelectedLine } from "@/lib/bindings";
import { Button } from "@/components/ui/button";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { fileViewReason, fileViewRev, mergeEditorReason, openBlame, openFileHistory, openMergeEditor } from "@/lib/fileViews";
import PanelHeader from "@/components/shell/PanelHeader.vue";
import FindBox from "@/components/diff/FindBox.vue";
import EmptyState from "@/components/shell/EmptyState.vue";
import { useFind, type FindHit } from "@/composables/useFind";
import { useVirtualRows } from "@/composables/useVirtualRows";
import { diffRowPx, diffTextStyle, displayLength, resolveDiffFont } from "@/lib/diff/font";
import { highlightPair, noDiffMessage, parseUnified, type DiffLine, type Segment } from "@/lib/diff/parseUnified";
import { findInText, glyphClass, markFound, scrollSidewaysTo } from "@/lib/diff/find";
import { pairByPosition, pairBySimilarity } from "@/lib/diff/pairLines";
import { BandBuilder, bandEdges, bandPath, sideOffsets, type Band } from "@/lib/diff/sideLayout";
import { highlightRuns, mergeSpans, spanRuns, spanStyle, themeFor, type SpanRun } from "@/lib/diff/syntax";
import type { RawToken } from "@/lib/diff/syntax.worker";
import { errorMessage, IpcError, unwrap } from "@/lib/ipc";
import { conflictComparisons, conflictStages, conflictSummary, missingVersionNote, operationNames, type ConflictComparison } from "@/lib/conflicts";
import type { FileItem } from "@/lib/files";
import { markResolved, openFileReason, openWorktreeFile, opFailureText, runOp, takeVersion } from "@/lib/repoActions";
import { cn } from "@/lib/utils";
import { useDialogsStore } from "@/stores/dialogs";
import { useSessionsStore, type RepoSession, type SelectedFile } from "@/stores/sessions";
import { useSettingsStore } from "@/stores/settings";
import { useUiStateStore } from "@/stores/uiState";

const props = withDefaults(
  defineProps<{
    session: RepoSession;
    /** The file to show in place of the session's selected file, as the File History dialog passes its commits' diffs. */
    file?: SelectedFile | null;
    /** Show the History and Blame buttons for the shown file in the header. */
    fileActions?: boolean;
  }>(),
  { file: undefined, fileActions: true },
);

const settings = useSettingsStore();
const uiState = useUiStateStore();
const sessions = useSessionsStore();
const dialogs = useDialogsStore();

const selected = computed<SelectedFile | null>(() => (props.file !== undefined ? props.file : props.session.selectedFile));

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
/** The layout the user chose with the Unified and Side by side buttons, remembered in the UI state. */
const chosenLayout = uiState.pref<Mode>((s) => s.diff?.layout, "unified", (layout) => ({ diff: { layout } }));
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
/**
 * The path of the file `diff` belongs to. The selected file changes as soon as another is
 * clicked, while `diff` keeps the previous file's text until the new one arrives; the syntax
 * colours follow this, so the old text never takes the new file's language meanwhile.
 */
const diffPath = ref<string | null>(null);
const loading = ref(false);
const error = ref<string | null>(null);
let request = 0;

async function load(): Promise<void> {
  const file = selected.value;
  const id = (request += 1);
  if (!file) {
    diff.value = null;
    diffPath.value = null;
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
    diffPath.value = file.path;
    scroller.value?.scrollTo({ top: 0 });
    resetSideScroll();
  } catch (e) {
    if (id !== request) return;
    diff.value = null;
    diffPath.value = null;
    error.value = errorMessage(e);
  } finally {
    if (id === request) loading.value = false;
  }
}

const loadDebounced = useDebounceFn(load, 50);

watch(
  () => [selected.value, contextLines.value, ignoreWhitespace.value, ignoreCrAtEol.value] as const,
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

/** One row on one side of the side-by-side layout, which shows each side's own lines only. */
interface SideItem {
  kind: "hunk" | "line" | "meta";
  hunk: number;
  header?: string;
  line?: DiffLine;
  pair?: number;
  text?: string;
}

interface Built {
  unified: UnifiedRow[];
  /** Side by side: the old file's lines, the new file's lines, and how they line up. */
  left: SideItem[];
  right: SideItem[];
  bands: Band[];
  /** Rows the side-by-side layout scrolls through, and the row of each hunk header. */
  sideRows: number;
  sideHunks: number[];
  pairs: Pair[];
  maxLen: number;
  /** Longest line on each side of the side-by-side view, in characters with tabs expanded. */
  leftMax: number;
  rightMax: number;
}

const built = computed<Built>(() => {
  const unified: UnifiedRow[] = [];
  const left: SideItem[] = [];
  const right: SideItem[] = [];
  const bands = new BandBuilder();
  const sideHunks: number[] = [];
  const pairs: Pair[] = [];
  let maxLen = 0;
  let leftMax = 0;
  let rightMax = 0;
  const done = (): Built => ({ unified, left, right, bands: bands.bands, sideRows: bands.rows, sideHunks, pairs, maxLen, leftMax, rightMax });
  const f = file.value;
  if (!f) return done();
  const tab = font.value.tabWidth;
  const matchSimilar = settings.settings?.diff?.matchSimilarLines !== false;

  f.hunks.forEach((hunk, h) => {
    unified.push({ kind: "hunk", hunk: h, header: hunk.header });
    sideHunks.push(bands.rows);
    left.push({ kind: "hunk", hunk: h, header: hunk.header });
    right.push({ kind: "hunk", hunk: h, header: hunk.header });
    bands.add("hunk", 1, 1);
    maxLen = Math.max(maxLen, hunk.header.length);

    let dels: DiffLine[] = [];
    let adds: DiffLine[] = [];
    // `\ No newline at end of file` lines sit between a del run and its add run; they must not
    // break the pairing, so they are held back and emitted after the run. Side by side, each
    // goes on the side of the line it is about, at the end of that side's run.
    let metas: DiffLine[] = [];
    let leftMetas: SideItem[] = [];
    let rightMetas: SideItem[] = [];
    let previous: DiffLine["kind"] | null = null;
    const flush = (): void => {
      const matched = matchSimilar
        ? pairBySimilarity(dels.map((d) => d.text), adds.map((a) => a.text))
        : pairByPosition(dels.length, adds.length);
      const delPair: (number | undefined)[] = [];
      const addPair: (number | undefined)[] = [];
      for (const [i, j] of matched) {
        delPair[i] = addPair[j] = pairs.length;
        pairs.push({ old: (dels[i] as DiffLine).text, new: (adds[j] as DiffLine).text });
      }
      for (const d of dels) leftMax = Math.max(leftMax, displayLength(d.text, tab));
      for (const a of adds) rightMax = Math.max(rightMax, displayLength(a.text, tab));
      dels.forEach((d, i) => unified.push({ kind: "line", hunk: h, line: d, pair: delPair[i] }));
      adds.forEach((a, j) => unified.push({ kind: "line", hunk: h, line: a, pair: addPair[j] }));
      for (const m of metas) unified.push({ kind: "line", hunk: h, line: m });
      dels.forEach((d, i) => left.push({ kind: "line", hunk: h, line: d, pair: delPair[i] }));
      adds.forEach((a, j) => right.push({ kind: "line", hunk: h, line: a, pair: addPair[j] }));
      left.push(...leftMetas);
      right.push(...rightMetas);
      // Side by side, one band per stretch: lines removed and added between two matched pairs,
      // then each run of matched pairs, so the curves join each line to its match. Without matching (or with nothing
      // matched) that is the whole block as one band. The notes about a missing final newline
      // go with the last stretch.
      const stretches: [number, number][] = [];
      let di = 0;
      let ai = 0;
      const unmatched = (toD: number, toA: number): void => {
        if (toD > di || toA > ai) stretches.push([toD - di, toA - ai]);
        di = toD;
        ai = toA;
      };
      if (matchSimilar) {
        let run = 0;
        const endRun = (): void => {
          if (run > 0) stretches.push([run, run]);
          run = 0;
        };
        for (const [i, j] of matched) {
          if (i > di || j > ai) {
            endRun();
            unmatched(i, j);
          }
          run += 1;
          di = i + 1;
          ai = j + 1;
        }
        endRun();
      }
      unmatched(dels.length, adds.length);
      const last = stretches[stretches.length - 1];
      if (last) {
        last[0] += leftMetas.length;
        last[1] += rightMetas.length;
      } else stretches.push([leftMetas.length, rightMetas.length]);
      const pure = dels.length === 0 || adds.length === 0;
      for (const [lh, rh] of stretches) bands.add("change", lh, rh, pure);
      dels = [];
      adds = [];
      metas = [];
      leftMetas = [];
      rightMetas = [];
    };

    for (const line of hunk.lines) {
      maxLen = Math.max(maxLen, displayLength(line.text, tab));
      if (line.kind === "del") {
        dels.push(line);
      } else if (line.kind === "add") {
        adds.push(line);
      } else if (line.kind === "meta") {
        metas.push(line);
        const item: SideItem = { kind: "meta", hunk: h, text: line.text };
        if (previous === "del") leftMetas.push(item);
        else if (previous === "add") rightMetas.push(item);
        else {
          // After an unchanged line: neither file ends with a newline.
          flush();
          left.push(item);
          right.push({ ...item });
          bands.add("same", 1, 1);
        }
      } else {
        flush();
        unified.push({ kind: "line", hunk: h, line });
        const len = displayLength(line.text, tab);
        leftMax = Math.max(leftMax, len);
        rightMax = Math.max(rightMax, len);
        left.push({ kind: "line", hunk: h, line });
        right.push({ kind: "line", hunk: h, line });
        bands.add("same", 1, 1);
      }
      if (line.kind !== "meta") previous = line.kind;
    }
    flush();
  });
  return done();
});

/**
 * Every line of the diff is added, or every line removed: an added or deleted file (an
 * untracked one included). Side by side, one of its sides would be empty.
 */
const oneSided = computed(() => {
  const kinds = new Set((file.value?.hunks ?? []).flatMap((h) => h.lines.map((l) => l.kind)).filter((k) => k !== "meta"));
  return kinds.size === 1 && (kinds.has("add") || kinds.has("del"));
});
/** The layout drawn: Unified for an added or deleted file whatever was chosen, else the choice. */
const mode = computed<Mode>(() => (oneSided.value ? "unified" : chosenLayout.value));

const rowCount = computed(() => (mode.value === "unified" ? built.value.unified.length : built.value.sideRows));

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

// ---- Syntax colours ----------------------------------------------------------------------------
//
// Each hunk's old side and new side go to the worker as runs of lines, so a string or comment
// that spans lines inside a hunk is coloured as one. Until the colours arrive, and for a file
// whose language is not known, the text is plain.

const preferredDark = usePreferredDark();
const dark = computed(() => settings.theme === "dark" || (settings.theme === "system" && preferredDark.value));
/** The path whose language colours the diff, or null when colouring is switched off. */
const syntaxPath = computed(() => (settings.settings?.diff?.syntaxHighlight === false ? null : diffPath.value));
/** Past this many lines a diff stays plain: colouring it would take longer than it is worth. */
const MAX_SYNTAX_LINES = 20_000;
/** Each line's tokens, filled in when the worker answers for the diff on screen. */
const lineTokens = shallowRef(new WeakMap<DiffLine, RawToken[]>());
let syntaxRequest = 0;

watch(
  [file, syntaxPath, dark],
  async () => {
    const id = (syntaxRequest += 1);
    lineTokens.value = new WeakMap();
    const f = file.value;
    const path = syntaxPath.value;
    if (!f || !path) return;
    const runs: string[][] = [];
    const owners: DiffLine[][] = [];
    let total = 0;
    for (const hunk of f.hunks) {
      const oldSide = hunk.lines.filter((l) => l.kind === "context" || l.kind === "del");
      const newSide = hunk.lines.filter((l) => l.kind === "context" || l.kind === "add");
      for (const side of [newSide, oldSide]) {
        if (side.length === 0) continue;
        runs.push(side.map((l) => l.text));
        owners.push(side);
        total += side.length;
      }
    }
    if (total === 0 || total > MAX_SYNTAX_LINES) return;
    try {
      const tokens = await highlightRuns(path, themeFor(dark.value), runs);
      if (id !== syntaxRequest || tokens === null) return;
      const map = new WeakMap<DiffLine, RawToken[]>();
      tokens.forEach((run, r) =>
        run.forEach((lineTokensOfRun, i) => {
          const owner = owners[r]?.[i];
          // A context line is in both sides' runs; the new side's colours come first and stay.
          if (owner && !map.has(owner)) map.set(owner, lineTokensOfRun);
        }),
      );
      lineTokens.value = map;
    } catch {
      // A language that fails to load leaves the diff plain.
    }
  },
  { immediate: true },
);

/**
 * A line's text as the rows draw it: syntax colours, Find matches, and each run of changed words
 * marked once. `key` names the row for Find: `u` and the row's index in the unified layout, or
 * `l` or `r` and the item's index on that side of the side-by-side layout.
 */
function runs(key: string, line: DiffLine | undefined, pair: number | undefined, sideOf: "old" | "new", fallback: string): SpanRun[] {
  const spans = mergeSpans(line ? lineTokens.value.get(line) : undefined, segments(pair, sideOf, fallback));
  return spanRuns(markFound(spans, find.ranges.value.get(key) ?? [], find.current.value));
}

// ---- Find (Ctrl+F) -------------------------------------------------------------------------
//
// Ctrl+F with the keyboard focus in the diff view opens a Find box in its top right corner
// (`useFind`, `FindBox.vue`). Every match is drawn in the find colour, the current one outlined.

const root = ref<HTMLElement | null>(null);

interface FindMatch extends FindHit {
  /** Its index in the unified rows, or among its side's items. */
  index: number;
  /** 0 for the unified layout, 1 for the left side and 2 for the right side. */
  side: 0 | 1 | 2;
}

/** The scroll row of the item at `index` on one side, inside the band that holds it. */
function sideRow(bands: Band[], index: number, side: "left" | "right"): number {
  let lo = 0;
  let hi = bands.length - 1;
  const start = (b: Band): number => (side === "left" ? b.l : b.r);
  while (lo < hi) {
    const mid = (lo + hi + 1) >> 1;
    if (start(bands[mid] as Band) <= index) lo = mid;
    else hi = mid - 1;
  }
  // Bands with no lines on this side share their start with the next one; take the one holding it.
  while (lo < bands.length - 1 && (side === "left" ? (bands[lo] as Band).lh : (bands[lo] as Band).rh) === 0) lo += 1;
  const b = bands[lo];
  if (!b) return 0;
  const count = side === "left" ? b.lh : b.rh;
  return b.y + (count > 0 ? ((index - start(b)) * b.h) / count : 0);
}

function collectMatches(query: string, max: number): FindMatch[] {
  const out: FindMatch[] = [];
  const add = (key: string, index: number, text: string, row: number, side: FindMatch["side"]): void => {
    for (const [start, end] of findInText(text, query)) {
      if (out.length >= max) return;
      out.push({ key, index, start, end, row, side });
    }
  };
  const b = built.value;
  if (mode.value === "unified") {
    b.unified.forEach((r, i) => {
      if (r.kind === "line" && r.line && r.line.kind !== "meta") add(`u${i}`, i, r.line.text, i, 0);
    });
    return out;
  }
  b.left.forEach((item, i) => {
    if (item.kind === "line" && item.line) add(`l${i}`, i, item.line.text, sideRow(b.bands, i, "left"), 1);
  });
  b.right.forEach((item, i) => {
    if (item.kind === "line" && item.line) add(`r${i}`, i, item.line.text, sideRow(b.bands, i, "right"), 2);
  });
  return out.sort((x, y) => x.row - y.row || x.side - y.side || x.start - y.start);
}

/** A match's top on screen (0 is the top of the rows) for a scroll position. */
function matchTop(m: FindMatch, scrollTop: number): number {
  const px = rowPx.value;
  if (m.side === 0) return m.index * px - scrollTop;
  const off = sideOffsets(built.value.bands, built.value.sideRows, px, scrollTop, virtual.viewportHeight.value);
  return (m.side === 1 ? off.left : off.right) + m.index * px - scrollTop;
}

/**
 * Scrolls a match into view, to the middle when it is off screen, then sideways when its text is
 * past the edge. Side by side, where a side's lines move at their own pace, the scroll position
 * is found by correcting it a few times.
 */
function revealMatch(m: FindMatch): void {
  const px = rowPx.value;
  const view = virtual.viewportHeight.value;
  const max = Math.max(0, rowCount.value * px - view);
  const clamp = (v: number): number => Math.min(max, Math.max(0, v));
  const top = matchTop(m, virtual.scrollTop.value);
  if (top < 0 || top + px > view) {
    const target = view / 2 - px / 2;
    let s = clamp(m.row * px - target);
    for (let i = 0; i < 4; i += 1) s = clamp(s + matchTop(m, s) - target);
    virtual.scrollTo(Math.round(s));
  }
  void nextTick(() => requestAnimationFrame(revealSideways));
}

function revealSideways(): void {
  const el = root.value?.querySelector<HTMLElement>("[data-find-current]");
  if (!el) return;
  const sideAttr = el.closest("[data-diff-side]")?.getAttribute("data-diff-side");
  const target = mode.value === "unified" ? scroller.value : sideAttr === "left" || sideAttr === "right" ? strip(sideAttr) : null;
  const area = mode.value === "unified" ? scroller.value : el.closest<HTMLElement>(".diff-side-code");
  if (target && area) scrollSidewaysTo(el, target, area, 0);
}

const find = useFind<FindMatch>({
  collect: collectMatches,
  topRow: () => virtual.scrollTop.value / rowPx.value,
  reveal: revealMatch,
  refocus: () => root.value?.focus({ preventScroll: true }),
  resetOn: [built, mode],
});

function onRootKeydown(event: KeyboardEvent): void {
  if (selected.value) find.onKeydown(event);
}

// ---- Virtualisation ------------------------------------------------------------------------

const scroller = ref<HTMLDivElement | null>(null);
const virtual = useVirtualRows(scroller, { rowHeight: () => rowPx.value, total: () => rowCount.value, overscan: 10 });

const visibleUnified = computed(() => built.value.unified.slice(virtual.start.value, virtual.end.value));
// ---- Side by side: each side's own lines, and the curves between them -----------------------

/** Width of the gap between the two sides, where the curves are drawn. */
const GAP_PX = 36;
const OVERSCAN = 10;

/** Where each side's first line sits in the scrolled content, for the current scroll position. */
const offsets = computed(() =>
  sideOffsets(built.value.bands, built.value.sideRows, rowPx.value, virtual.scrollTop.value, virtual.viewportHeight.value),
);

interface SideView {
  items: SideItem[];
  /** Index of the first item drawn, and the pixel offset of the block holding them. */
  start: number;
  top: number;
}

function sideView(items: SideItem[], offset: number): SideView {
  const px = rowPx.value;
  const top = virtual.scrollTop.value;
  const first = Math.max(0, Math.floor((top - offset) / px) - OVERSCAN);
  const last = Math.min(items.length, Math.ceil((top + virtual.viewportHeight.value - offset) / px) + OVERSCAN);
  return { items: items.slice(first, Math.max(first, last)), start: first, top: offset + first * px };
}

const leftView = computed(() => sideView(built.value.left, offsets.value.left));
const rightView = computed(() => sideView(built.value.right, offsets.value.right));

interface Curve {
  key: number;
  /** The filled band, and its top and bottom edges. */
  d: string;
  edges: string;
  kind: "del" | "add" | "both" | "hunk";
  /** Draw its top and bottom edges: only a whole insertion or deletion has them. */
  outline: boolean;
}

/**
 * The top and bottom of a band's outline on one side. They sit on the half pixel, so each 1px
 * edge fills one row of pixels instead of two half rows. A side with no lines comes to a point
 * on the half pixel too, where the line marking the insertion is drawn, so the two meet exactly.
 */
function sideEdges(y: number, height: number): [number, number] {
  return height === 0 ? [y + 0.5, y + 0.5] : [y + 0.5, y + height - 0.5];
}

/**
 * The top and bottom of a band's fill on one side: exactly its lines, so neighbouring bands of
 * one block meet with no gap between them. An outlined band's point sits on the half pixel, on
 * the line marking where its lines would go.
 */
function sideFill(y: number, height: number, outline: boolean): [number, number] {
  if (height === 0) return outline ? [y + 0.5, y + 0.5] : [y, y];
  return [y, y + height];
}

/** The bands to draw in the gap, in the gap's own coordinates (0 is the top of the view). */
const curves = computed<Curve[]>(() => {
  const px = rowPx.value;
  const top = virtual.scrollTop.value;
  const view = virtual.viewportHeight.value;
  const { left, right } = offsets.value;
  const out: Curve[] = [];
  built.value.bands.forEach((b, i) => {
    if (b.kind === "same") return;
    const outline = b.outline === true;
    const ly = left + b.l * px - top;
    const ry = right + b.r * px - top;
    const [lt, lb] = sideFill(ly, b.lh * px, outline);
    const [rt, rb] = sideFill(ry, b.rh * px, outline);
    if (Math.max(lb, rb) < 0 || Math.min(lt, rt) > view) return;
    const kind = b.kind === "hunk" ? "hunk" : b.lh === 0 ? "add" : b.rh === 0 ? "del" : "both";
    const [elt, elb] = sideEdges(ly, b.lh * px);
    const [ert, erb] = sideEdges(ry, b.rh * px);
    out.push({ key: i, d: bandPath(GAP_PX, lt, lb, rt, rb), edges: bandEdges(GAP_PX, elt, elb, ert, erb), kind, outline });
  });
  return out;
});

/** The two columns of the side-by-side layout, left of the gap and right of it. */
const sideColumns = computed(() => [
  { side: "left" as const, left: 0, width: leftTextPx.value, view: leftView.value, change: "del" as const },
  { side: "right" as const, left: leftTextPx.value + GAP_PX, width: rightTextPx.value, view: rightView.value, change: "add" as const },
]);

/** Unique per viewer, since the File History dialog shows a second one. */
const gradientId = `diff-curve-${useId()}`;

/**
 * Where a change has no lines on one side, a line across that side marks where they would go,
 * leading into the curve. In the gap's coordinates, as for the curves.
 */
const insertMarks = computed<{ key: number; side: Side; y: number }[]>(() => {
  const px = rowPx.value;
  const top = virtual.scrollTop.value;
  const view = virtual.viewportHeight.value;
  const out: { key: number; side: Side; y: number }[] = [];
  built.value.bands.forEach((b, i) => {
    if (b.kind !== "change" || !b.outline || (b.lh > 0 && b.rh > 0)) return;
    const side: Side = b.lh === 0 ? "left" : "right";
    const y = (side === "left" ? offsets.value.left + b.l * px : offsets.value.right + b.r * px) - top;
    if (y >= -1 && y <= view + 1) out.push({ key: i, side, y });
  });
  return out;
});

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

/** Where the divider sits: the middle of the gap. Each side's text ends half a gap short of it. */
const leftPx = computed(() => Math.round(Math.min(SPLIT_MAX, Math.max(SPLIT_MIN, split.value)) * rowsWidth.value));
const rightPx = computed(() => Math.max(0, rowsWidth.value - leftPx.value));
const leftTextPx = computed(() => Math.max(0, leftPx.value - GAP_PX / 2));
const rightTextPx = computed(() => Math.max(0, rightPx.value - GAP_PX / 2));
const sideStyle = computed(() => ({ "--left-w": `${leftTextPx.value}px`, "--right-w": `${rightTextPx.value}px` }));

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
  if (mode.value === "side") return built.value.sideHunks;
  const out: number[] = [];
  built.value.unified.forEach((r, i) => {
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

/** Why the shown file's history and blame cannot be opened, or null. */
const fileViewBlocked = computed(() => fileViewReason(selected.value?.target ?? null));

function showHistory(): void {
  const f = selected.value;
  if (f && !fileViewBlocked.value) openFileHistory(props.session.key, f.path, fileViewRev(f.target));
}

function showBlame(): void {
  const f = selected.value;
  if (f && !fileViewBlocked.value) openBlame(props.session.key, f.path, fileViewRev(f.target));
}

/** The header's buttons for the shown file, with the tooltip each shows when it can be used. */
const FILE_VIEW_BUTTONS = [
  { label: "History", title: "Every commit that changed this file", icon: HistoryIcon, run: showHistory },
  { label: "Blame", title: "The commit that last changed each line of this file", icon: ScanTextIcon, run: showBlame },
];

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
const sideNames = computed(() => operationNames(props.session.status, props.session.refs, props.session.stashConflict));
const comparisons = computed(() => conflictComparisons(sideNames.value));
const openReason = computed(() => openFileReason(props.session));

function isComparing(c: ConflictComparison): boolean {
  const t = selected.value?.target;
  return t?.type === "conflict" && t.from === c.from && t.to === c.to;
}

/** Why a comparison has nothing on one side for this file, or null. */
function comparisonNote(c: ConflictComparison): string | null {
  const f = conflictFile.value;
  if (!f) return null;
  const missing = [c.from, c.to].find((v) => v !== "workingTree" && !f.stages[v]);
  return missing && missing !== "workingTree" ? missingVersionNote(f.letters, missing, sideNames.value) : null;
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
  if (f && !f.stages[side]) {
    const why = f.letters[side === "ours" ? 0 : 1] === "D" ? "deleted this file" : "does not have this file";
    return `${name} ${why}, so taking its version deletes the file and marks the conflict resolved.`;
  }
  return `Replace the file with ${name}'s version, throwing away the other side's changes and any edits to the file, and mark the conflict resolved.`;
}

function toggleContext(value: string): void {
  const n = Number(value);
  if (Number.isFinite(n) && n > 0) contextLines.value = n;
}
</script>

<template>
  <section ref="root" class="area flex h-full min-h-0 flex-col outline-none" tabindex="-1" @keydown="onRootKeydown">
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
          <template v-if="fileActions">
            <Tooltip v-for="action in FILE_VIEW_BUTTONS" :key="action.label">
              <TooltipTrigger as-child>
                <!-- A disabled button swallows pointer events, so the wrapper span carries the tooltip. -->
                <span class="inline-flex">
                  <button
                    type="button"
                    class="toggle inline-flex h-[18px] items-center gap-1 rounded-sm border px-2 leading-none font-medium disabled:opacity-50 [&_svg]:size-3"
                    :disabled="fileViewBlocked !== null"
                    @click="action.run"
                  >
                    <component :is="action.icon" />
                    {{ action.label }}
                  </button>
                </span>
              </TooltipTrigger>
              <TooltipContent side="bottom" class="max-w-72">{{ fileViewBlocked ?? action.title }}</TooltipContent>
            </Tooltip>
            <span class="mx-0.5 h-3 w-px bg-border-soft" aria-hidden="true" />
          </template>
          <div class="inline-flex overflow-hidden rounded-md border border-border-soft">
            <button
              type="button"
              :aria-pressed="mode === 'unified'"
              class="toggle h-[18px] px-2 leading-none font-medium rounded-none border-0"
              @click="chosenLayout = 'unified'"
            >
              Unified
            </button>
            <!-- An added or deleted file is always unified; the choice is kept for the next file. -->
            <button
              type="button"
              :aria-pressed="mode === 'side'"
              :disabled="oneSided"
              :title="oneSided ? 'An added or deleted file is shown unified: side by side, one side would be empty' : undefined"
              class="toggle h-[18px] px-2 leading-none font-medium rounded-none border-0 border-l border-l-border-soft disabled:opacity-50"
              data-layout-side
              @click="chosenLayout = 'side'"
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
        <button
          type="button"
          :class="cn('diff-action', mergeEditorReason(conflictFile.letters) && 'opacity-50')"
          :title="mergeEditorReason(conflictFile.letters) ?? 'Resolve this file block by block in the three-pane merge editor'"
          :aria-disabled="mergeEditorReason(conflictFile.letters) !== null"
          data-resolve="editor"
          @click="!mergeEditorReason(conflictFile.letters) && openMergeEditor(session.key, conflictFile.item.path)"
        >
          Merge Editor…
        </button>
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
    <EmptyState v-else-if="diff && rowCount === 0 && !loading" :message="noDiffMessage(diff.note, parsed)" />

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
      <!-- The Find box (Ctrl+F), clear of the vertical scrollbar. -->
      <FindBox
        v-if="find.open.value"
        :ref="(el) => (find.box.value = el as InstanceType<typeof FindBox> | null)"
        v-model="find.query.value"
        class="absolute top-1.5 right-5 z-20"
        :summary="find.summary.value"
        :can-step="find.matches.value.length > 0"
        placeholder="Find in this diff"
        @step="find.step"
        @close="find.close"
      />
      <div
        ref="scroller"
        data-diff-rows
        :class="cn('relative min-h-0 flex-1 select-text scrollbar-thin', mode === 'side' ? 'overflow-x-hidden overflow-y-auto' : 'overflow-auto')"
        :style="textStyle"
        @wheel="mode === 'side' ? onSideWheel($event) : undefined"
      >
        <div class="relative" :style="mode === 'side' ? { height: `${rowCount * rowPx}px` } : { height: `${rowCount * rowPx}px`, minWidth: contentMinWidth }">
          <!-- No will-change-transform here: it would put the rows on a compositor layer of their own,
               where Chromium never draws ClearType, so the text would come out grayscale and soft
               whatever diff.fontSmoothing says. See smoothingStyle in lib/diff/font.ts. -->
          <div v-if="mode === 'unified'" class="absolute inset-x-0 top-0" :style="{ transform: `translateY(${virtual.start.value * rowPx}px)` }">
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
                    <template v-for="(run, i) in runs(`u${virtual.start.value + k}`, row.line, row.pair, row.line?.kind === 'del' ? 'old' : 'new', row.line?.text ?? '')" :key="i">
                      <mark v-if="run.changed" :class="row.line?.kind === 'del' ? 'diff-word-del' : 'diff-word-add'"><span v-for="(seg, j) in run.spans" :key="j" :class="glyphClass(seg)" :style="spanStyle(seg)" :data-find-current="seg.found === 2 ? '' : undefined">{{ seg.text }}</span></mark>
                      <template v-else><span v-for="(seg, j) in run.spans" :key="j" :class="glyphClass(seg)" :style="spanStyle(seg)" :data-find-current="seg.found === 2 ? '' : undefined">{{ seg.text }}</span></template>
                    </template>
                  </span>
                </div>
              </template>
            </template>

          </div>

          <!-- Side by side: each side's own lines in a column of its own, placed by sideOffsets. -->
          <template v-if="mode === 'side'">
            <div
              v-for="col in sideColumns"
              :key="col.side"
              class="absolute top-0"
              :style="{ left: `${col.left}px`, width: `${col.width}px`, transform: `translateY(${col.view.top}px)` }"
              :data-diff-side="col.side"
            >
              <template v-for="(item, k) in col.view.items" :key="col.view.start + k">
                <div v-if="item.kind === 'hunk'" class="diff-hunk overflow-hidden">
                  <template v-if="col.side === 'left'">
                    <span v-if="lineMode" class="diff-hunk-actions">
                      <template v-if="lineMode === 'stage'">
                        <button type="button" class="diff-action" :disabled="busy" @click="applyLines('stage', hunkLines(item.hunk), 'this hunk')">Stage hunk</button>
                        <button type="button" class="diff-action diff-action-destructive" :disabled="busy" @click="applyLines('discard', hunkLines(item.hunk), 'this hunk')">Discard…</button>
                      </template>
                      <button v-else type="button" class="diff-action" :disabled="busy" @click="applyLines('unstage', hunkLines(item.hunk), 'this hunk')">Unstage hunk</button>
                    </span>
                    <span class="diff-glyphs"> {{ item.header }}</span>
                  </template>
                </div>
                <div v-else-if="item.kind === 'meta'" class="diff-side-meta">
                  <span class="diff-glyphs"> \ {{ item.text }}</span>
                </div>
                <div
                  v-else-if="item.line"
                  :class="['diff-line min-w-0 overflow-hidden', lineClass(item.line.kind === 'context' ? 'context' : col.change), item.line.kind !== 'context' && isPicked(item.line) && 'diff-line-picked']"
                >
                  <span
                    :class="['diff-num', item.line.kind !== 'context' && canPick(item.line) && 'diff-num-pick']"
                    @mousedown="item.line.kind !== 'context' && onPick(item.line, $event)"
                  ><span class="diff-glyphs">{{ (col.side === 'left' ? item.line.oldNo : item.line.newNo) ?? "" }}</span></span>
                  <span class="diff-sign">{{ marker(item.line.kind === 'context' ? 'context' : col.change) }}</span>
                  <span :class="['diff-side-code', col.side === 'left' ? 'diff-text-left' : 'diff-text-right']"><span>
                    <template v-for="(run, i) in runs(`${col.side === 'left' ? 'l' : 'r'}${col.view.start + k}`, item.line, item.pair, col.side === 'left' ? 'old' : 'new', item.line.text)" :key="i">
                      <mark v-if="run.changed" :class="col.side === 'left' ? 'diff-word-del' : 'diff-word-add'"><span v-for="(seg, j) in run.spans" :key="j" :class="glyphClass(seg)" :style="spanStyle(seg)" :data-find-current="seg.found === 2 ? '' : undefined">{{ seg.text }}</span></mark>
                      <template v-else><span v-for="(seg, j) in run.spans" :key="j" :class="glyphClass(seg)" :style="spanStyle(seg)" :data-find-current="seg.found === 2 ? '' : undefined">{{ seg.text }}</span></template>
                    </template>
                  </span></span>
                </div>
              </template>
            </div>
          </template>
        </div>
      </div>

      <!-- Side by side: the curves joining each change's old lines to its new lines, over the rows
           so they line up with them, and clipped to the rows so a band scrolled above them does
           not draw over the diff panel's header. Inline SVG rather than a canvas: a canvas gets a compositor
           layer of its own, which would stop Chromium drawing ClearType on the text near it. -->
      <svg
        v-if="mode === 'side' && rowsWidth > 0"
        class="pointer-events-none absolute top-0 left-0 overflow-hidden"
        :width="rowsWidth"
        :height="virtual.viewportHeight.value"
        aria-hidden="true"
        data-diff-curves
      >
        <defs>
          <!-- In the gap's own units: the default units follow each shape's box, which has no
               height for a level edge, and the gradient would then not draw it at all. -->
          <!-- From the deleted lines' background to the added lines', so the band joins each end's
               lines without a seam. -->
          <linearGradient :id="gradientId" gradientUnits="userSpaceOnUse" x1="0" :x2="GAP_PX" y1="0" y2="0">
            <stop offset="0" class="diff-curve-stop-del" />
            <stop offset="1" class="diff-curve-stop-add" />
          </linearGradient>
          <!-- A band's edges are solid where they meet the line marking an insertion or deletion,
               and fade out towards the block of lines at the other end, which needs no outline. -->
          <linearGradient :id="`${gradientId}-edge-add`" gradientUnits="userSpaceOnUse" x1="0" :x2="GAP_PX" y1="0" y2="0">
            <stop offset="0" class="diff-curve-edge-stop-add" />
            <stop offset="1" class="diff-curve-edge-stop-add diff-curve-edge-stop-clear" />
          </linearGradient>
          <linearGradient :id="`${gradientId}-edge-del`" gradientUnits="userSpaceOnUse" x1="0" :x2="GAP_PX" y1="0" y2="0">
            <stop offset="0" class="diff-curve-edge-stop-del diff-curve-edge-stop-clear" />
            <stop offset="1" class="diff-curve-edge-stop-del" />
          </linearGradient>
        </defs>
        <line
          v-for="m in insertMarks"
          :key="`m${m.key}`"
          :class="m.side === 'left' ? 'diff-curve-mark-add' : 'diff-curve-mark-del'"
          :x1="m.side === 'left' ? 0 : leftTextPx + GAP_PX"
          :x2="m.side === 'left' ? leftTextPx : rowsWidth"
          :y1="m.y + 0.5"
          :y2="m.y + 0.5"
        />
        <g :transform="`translate(${leftTextPx} 0)`">
          <template v-for="c in curves" :key="c.key">
            <path :d="c.d" :class="`diff-curve-${c.kind}`" :style="c.kind === 'both' ? { fill: `url(#${gradientId})` } : undefined" />
            <path
              v-if="c.outline && (c.kind === 'add' || c.kind === 'del')"
              :d="c.edges"
              class="diff-curve-edge"
              :style="{ stroke: `url(#${gradientId}-edge-${c.kind})` }"
            />
          </template>
        </g>
      </svg>
      <!-- Side by side: one horizontal scrollbar under each side's text, level with the rows above. -->
      <div v-if="mode === 'side'" class="flex shrink-0" :style="[textStyle, { paddingRight: `${scrollbarPx}px`, lineHeight: 'normal' }]">
        <div class="flex" :style="{ width: `${leftTextPx}px`, marginRight: `${GAP_PX}px` }">
          <div class="diff-gutter-space shrink-0" />
          <div ref="stripLeft" class="min-w-0 flex-1 overflow-x-auto overflow-y-hidden scrollbar-thin" @scroll="onStripScroll('left')">
            <div class="h-px" :style="{ width: `calc(${built.leftMax}ch + 0.5rem)` }" />
          </div>
        </div>
        <div class="flex" :style="{ width: `${rightTextPx}px` }">
          <div class="diff-gutter-space shrink-0" />
          <div ref="stripRight" class="min-w-0 flex-1 overflow-x-auto overflow-y-hidden scrollbar-thin" @scroll="onStripScroll('right')">
            <div class="h-px" :style="{ width: `calc(${built.rightMax}ch + 0.5rem)` }" />
          </div>
        </div>
      </div>

      <!-- Divider between the two sides: drag to move it, double-click to go back to half and half.
           It takes the whole gap between the sides, whose curves ignore the mouse, so the resize
           cursor shows anywhere between them. Its line shows only on hover and while dragging,
           so it does not cut through the curves. -->
      <button
        v-if="mode === 'side' && rowsWidth > 0"
        type="button"
        class="group absolute top-0 bottom-0 z-10 flex cursor-col-resize justify-center outline-none"
        :style="{ left: `${leftPx - GAP_PX / 2}px`, width: `${GAP_PX}px` }"
        aria-label="Resize the two sides of the diff"
        title="Drag to resize the two sides; double-click to split them evenly"
        @pointerdown="startSplitDrag"
        @dblclick="split = 0.5"
      >
        <span :class="['h-full w-px transition-colors', splitDragging ? 'w-[2px] bg-primary' : 'bg-transparent group-hover:w-[2px] group-hover:bg-primary/70']" />
      </button>
      </div>
    </template>
  </section>
</template>

