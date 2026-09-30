<script setup lang="ts">
import { remToPx } from "@/lib/graph/paths";
import { computed, ref, shallowRef, watch } from "vue";
import { useDebounceFn, useLocalStorage, useResizeObserver } from "@vueuse/core";
import { ChevronDownIcon, ChevronUpIcon, LoaderCircleIcon } from "@lucide/vue";
import { commands, type DiffText } from "@/lib/bindings";
import { Button } from "@/components/ui/button";
import PanelHeader from "@/components/shell/PanelHeader.vue";
import EmptyState from "@/components/shell/EmptyState.vue";
import { useVirtualRows } from "@/composables/useVirtualRows";
import { highlightPair, parseUnified, type DiffLine, type Segment } from "@/lib/diff/parseUnified";
import { errorMessage, unwrap } from "@/lib/ipc";
import { cn } from "@/lib/utils";
import type { RepoSession } from "@/stores/sessions";
import { useSettingsStore } from "@/stores/settings";

const props = defineProps<{
  session: RepoSession;
}>();

const settings = useSettingsStore();

/** Diff text size (Tailwind `text-xs`) in pixels. */
const FONT_PX = remToPx(0.75);
/**
 * Row height in whole pixels: the diff font size times `appearance.diffLineHeight` from
 * settings.json. The virtual list and the rows both use this one number, so rows never drift from
 * where the list expects them.
 */
const rowPx = computed(() => {
  const factor = settings.settings?.appearance?.diffLineHeight ?? 1.2;
  const clamped = Number.isFinite(factor) ? Math.min(3, Math.max(1, factor)) : 1.2;
  return Math.max(1, Math.round(FONT_PX * clamped));
});
const ALL_CONTEXT = 99999;

type Mode = "unified" | "side";
const mode = useLocalStorage<Mode>("stashpanda.diff.mode", "unified");
const contextLines = useLocalStorage<number>("stashpanda.diff.context", 3);
const ignoreWhitespace = useLocalStorage<boolean>("stashpanda.diff.ignoreWhitespace", false);
const ignoreCrAtEol = useLocalStorage<boolean>("stashpanda.diff.ignoreCrAtEol", false);
/** Share of the side-by-side width given to the old (left) side. */
const split = useLocalStorage<number>("stashpanda.diff.split", 0.5);

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

/** Display width of a line in characters, counting a tab as up to 8 columns (the browser default). */
function displayLength(text: string): number {
  let col = 0;
  for (const ch of text) col += ch === "\t" ? 8 - (col % 8) : 1;
  return col;
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
        if (d) leftMax = Math.max(leftMax, displayLength(d.text));
        if (a) rightMax = Math.max(rightMax, displayLength(a.text));
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
      maxLen = Math.max(maxLen, line.text.length);
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
      const len = displayLength(line.text);
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
  if (pair === undefined) return [{ text: fallback, changed: false }];
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

/** Gutter (two line numbers plus marker) in pixels, then the widest line in `ch`. */
const GUTTER_PX = 92;
/** Unified only: the rows are as wide as the longest line and the whole view scrolls sideways. */
const contentMinWidth = computed(() => `calc(${GUTTER_PX}px + ${Math.max(20, built.value.maxLen + 2)}ch)`);

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
const headerPath = computed(() => {
  const s = selected.value;
  if (!s) return "";
  return s.oldPath && s.oldPath !== s.path ? `${s.oldPath} → ${s.path}` : s.path;
});

function lineClass(kind: DiffLine["kind"] | undefined): string {
  switch (kind) {
    case "add":
      return "border-l-2 border-transparent bg-diff-add";
    case "del":
      return "border-l-2 border-transparent bg-diff-del";
    case "meta":
      return "border-l-2 border-transparent text-muted-foreground italic";
    default:
      return "border-l-2 border-transparent";
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

function toggleContext(value: string): void {
  const n = Number(value);
  if (Number.isFinite(n) && n > 0) contextLines.value = n;
}
</script>

<template>
  <section class="area flex h-full min-h-0 flex-col" :style="{ '--diff-row-h': `${rowPx}px` }">
    <PanelHeader title="Diff">
      <span v-if="selected" class="truncate font-mono text-xs text-foreground" :title="headerPath">{{ headerPath }}</span>
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

      <div class="relative flex min-h-0 flex-1 flex-col" :style="mode === 'side' ? sideStyle : undefined">
      <div
        ref="scroller"
        :class="cn('relative min-h-0 flex-1 font-mono text-xs leading-(--diff-row-h) scrollbar-thin', mode === 'side' ? 'overflow-x-hidden overflow-y-auto' : 'overflow-auto')"
        @wheel="mode === 'side' ? onSideWheel($event) : undefined"
      >
        <div class="relative" :style="mode === 'side' ? { height: `${rowCount * rowPx}px` } : { height: `${rowCount * rowPx}px`, minWidth: contentMinWidth }">
          <div class="absolute inset-x-0 top-0 will-change-transform" :style="{ transform: `translateY(${virtual.start.value * rowPx}px)` }">
            <!-- Unified -->
            <template v-if="mode === 'unified'">
              <template v-for="(row, k) in visibleUnified" :key="virtual.start.value + k">
                <div v-if="row.kind === 'hunk'" class="flex h-(--diff-row-h) items-center bg-diff-hunk px-2 whitespace-pre text-diff-hunk-fg">
                  {{ row.header }}
                </div>
                <div v-else :class="cn('flex h-(--diff-row-h) whitespace-pre', lineClass(row.line?.kind))">
                  <span class="w-10 shrink-0 bg-diff-gutter pr-1 text-right text-gutter-fg tabular-nums shadow-[inset_1px_0_0_rgba(0,0,0,0.25)] select-none">{{ row.line?.oldNo ?? "" }}</span>
                  <span class="w-10 shrink-0 bg-diff-gutter pr-1 text-right text-gutter-fg tabular-nums shadow-[inset_1px_0_0_rgba(0,0,0,0.25)] select-none">{{ row.line?.newNo ?? "" }}</span>
                  <span class="w-3 shrink-0 text-center text-muted-foreground select-none">{{ marker(row.line?.kind) }}</span>
                  <span class="flex-1 pr-2">
                    <template v-for="(seg, i) in segments(row.pair, row.line?.kind === 'del' ? 'old' : 'new', row.line?.text ?? '')" :key="i">
                      <mark v-if="seg.changed" :class="row.line?.kind === 'del' ? 'rounded-sm bg-diff-del-mark text-inherit' : 'rounded-sm bg-diff-add-mark text-inherit'">{{ seg.text }}</mark>
                      <template v-else>{{ seg.text }}</template>
                    </template>
                  </span>
                </div>
              </template>
            </template>

            <!-- Side by side -->
            <template v-else>
              <template v-for="(row, k) in visibleSide" :key="virtual.start.value + k">
                <div v-if="row.kind === 'hunk'" class="flex h-(--diff-row-h) items-center overflow-hidden bg-diff-hunk px-2 whitespace-pre text-diff-hunk-fg">
                  {{ row.header }}
                </div>
                <div v-else-if="row.kind === 'meta'" class="flex h-(--diff-row-h) items-center overflow-hidden border-l-2 border-transparent px-11 whitespace-pre text-muted-foreground italic">
                  \ {{ row.text }}
                </div>
                <div v-else class="diff-side-row h-(--diff-row-h)">
                  <div :class="cn('flex min-w-0 overflow-hidden whitespace-pre', row.left ? lineClass(row.left.kind === 'context' ? 'context' : 'del') : 'border-l-2 border-transparent bg-muted/20')">
                    <span class="w-10 shrink-0 bg-diff-gutter pr-1 text-right text-gutter-fg tabular-nums shadow-[inset_1px_0_0_rgba(0,0,0,0.25)] select-none">{{ row.left?.oldNo ?? "" }}</span>
                    <span class="w-3 shrink-0 text-center text-muted-foreground select-none">{{ row.left ? marker(row.left.kind === 'context' ? 'context' : 'del') : "" }}</span>
                    <span class="diff-text-left min-w-0 flex-1 overflow-hidden"><span class="inline-block pr-2">
                      <template v-if="row.left">
                        <template v-for="(seg, i) in segments(row.pair, 'old', row.left.text)" :key="i">
                          <mark v-if="seg.changed" class="rounded-sm bg-diff-del-mark text-inherit">{{ seg.text }}</mark>
                          <template v-else>{{ seg.text }}</template>
                        </template>
                      </template>
                    </span></span>
                  </div>
                  <div :class="cn('flex min-w-0 overflow-hidden whitespace-pre', row.right ? lineClass(row.right.kind === 'context' ? 'context' : 'add') : 'border-l-2 border-transparent bg-muted/20')">
                    <span class="w-10 shrink-0 bg-diff-gutter pr-1 text-right text-gutter-fg tabular-nums shadow-[inset_1px_0_0_rgba(0,0,0,0.25)] select-none">{{ row.right?.newNo ?? "" }}</span>
                    <span class="w-3 shrink-0 text-center text-muted-foreground select-none">{{ row.right ? marker(row.right.kind === 'context' ? 'context' : 'add') : "" }}</span>
                    <span class="diff-text-right min-w-0 flex-1 overflow-hidden"><span class="inline-block pr-2">
                      <template v-if="row.right">
                        <template v-for="(seg, i) in segments(row.pair, 'new', row.right.text)" :key="i">
                          <mark v-if="seg.changed" class="rounded-sm bg-diff-add-mark text-inherit">{{ seg.text }}</mark>
                          <template v-else>{{ seg.text }}</template>
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
      <div v-if="mode === 'side'" class="flex shrink-0 font-mono text-xs" :style="{ paddingRight: `${scrollbarPx}px` }">
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

<style>
/* Side-by-side rows: the two sides take the widths the divider sets. */
.diff-side-row {
  display: grid;
  grid-template-columns: var(--left-w, 50%) var(--right-w, 50%);
}
/* Each side's text moves by its own horizontal scroll; the line numbers stay where they are. */
.diff-text-left > span {
  transform: translateX(calc(-1 * var(--sx-left, 0px)));
}
.diff-text-right > span {
  transform: translateX(calc(-1 * var(--sx-right, 0px)));
}
/* Same width as a side's gutter (2px border, 2.5rem line number, 0.75rem marker), so each
   horizontal scrollbar starts under its text. */
.diff-gutter-space {
  width: calc(2px + 3.25rem);
}
</style>
