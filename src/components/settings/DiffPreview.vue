<script lang="ts">
import { ref } from "vue";

/** The preview's layout and height, kept while the app runs but not saved. */
const layout = ref<"unified" | "side">("side");
const height = ref(216);
const MIN_HEIGHT = 80;
const MAX_HEIGHT = 600;
</script>

<script setup lang="ts">
import { computed, shallowRef, useId, watch } from "vue";
import { usePreferredDark } from "@vueuse/core";
import type { FontSmoothing } from "@/lib/bindings";
import { diffColorVars } from "@/lib/diff/colors";
import { diffRowPx, diffTextStyle, resolveDiffFont } from "@/lib/diff/font";
import { highlightPair, type HighlightedPair, type Segment } from "@/lib/diff/parseUnified";
import { bandEdges, bandPath, GAP_PX, sideEdges, sideFill } from "@/lib/diff/sideLayout";
import { highlightRuns, insertGaps, mergeSpans, spanRuns, spanStyle, themeFor, type SpanRun } from "@/lib/diff/syntax";
import { glyphClass } from "@/lib/diff/find";
import type { RawToken } from "@/lib/diff/syntax.worker";
import { useSettingsStore } from "@/stores/settings";
import type { FormValue } from "./sections";

/**
 * A few diff rows drawn the way `DiffViewer.vue` draws them, unified or side by side, using the
 * Diff page's values before they are saved. A value that cannot be saved yet falls back to its
 * default here.
 */
const props = defineProps<{
  /** The dialog's form values, keyed `diff.fontSize` and so on. */
  values: Record<string, FormValue>;
}>();

function num(key: string): number | undefined {
  const text = String(props.values[`diff.${key}`] ?? "").trim();
  return text.length > 0 ? Number(text) : undefined;
}

const font = computed(() =>
  resolveDiffFont({
    fontFamily: String(props.values["diff.fontFamily"] ?? ""),
    fontSize: num("fontSize"),
    lineHeight: num("lineHeight"),
    tabWidth: num("tabWidth"),
    ligatures: props.values["diff.ligatures"] === true,
    fontSmoothing: props.values["diff.fontSmoothing"] as FontSmoothing,
  }),
);
const wordHighlight = computed(() => props.values["diff.wordHighlight"] === true);
const modLeftAsRemoved = computed(() => props.values["diff.modLeftAsRemoved"] === true);

type Kind = "context" | "add" | "del";
interface Row {
  kind: Kind;
  oldNo: number | null;
  newNo: number | null;
  text: string;
  /** The other half of a changed pair, for word highlighting. */
  other?: string;
}

const OLD_GREETING = '\tconst greeting = "Hello";';
const NEW_GREETING = '\tconst greeting = "Good morning";';
// A word removed with nothing in its place: side by side, a cursor marks where it was.
const OLD_LOG = "\tlog(greeting, names, options);";
const NEW_LOG = "\tlog(greeting, options);";
const OLD_MAP = "\t\treturn names.map((n) => `${greeting}, ${n}`);";
const NEW_MAP = "\t\treturn names.map((n) => `${greeting}, ${n}!`);";

// Two modified blocks, then an insertion and a deletion, so that every diff colour shows.
const HUNK = "@@ -12,9 +12,9 @@ export function greet(names, options) {";
const ROWS: Row[] = [
  { kind: "context", oldNo: 12, newNo: 12, text: "export function greet(names, options) {" },
  { kind: "del", oldNo: 13, newNo: null, text: OLD_GREETING, other: NEW_GREETING },
  { kind: "del", oldNo: 14, newNo: null, text: OLD_LOG, other: NEW_LOG },
  { kind: "add", oldNo: null, newNo: 13, text: NEW_GREETING, other: OLD_GREETING },
  { kind: "add", oldNo: null, newNo: 14, text: NEW_LOG, other: OLD_LOG },
  { kind: "context", oldNo: 15, newNo: 15, text: "\tif (names !== undefined && names.length >= 1) {" },
  { kind: "del", oldNo: 16, newNo: null, text: OLD_MAP, other: NEW_MAP },
  { kind: "add", oldNo: null, newNo: 16, text: NEW_MAP, other: OLD_MAP },
  { kind: "context", oldNo: 17, newNo: 17, text: "\t}" },
  { kind: "add", oldNo: null, newNo: 18, text: '\tconsole.warn("No names given");' },
  { kind: "context", oldNo: 18, newNo: 19, text: '\treturn greeting + " -> nobody";' },
  { kind: "del", oldNo: 19, newNo: null, text: "\t// TODO: say it in other languages" },
  { kind: "context", oldNo: 20, newNo: 20, text: "}" },
];

// ---- Side by side ------------------------------------------------------------------------------

/** One line on one side, by its index in `ROWS`. */
interface SideLine {
  index: number;
  /** Part of a block with both removed and added lines. */
  modified: boolean;
}

/** A block of changed lines: where it starts on each side (counting the hunk header) and its lines there. */
interface SideBlock {
  l: number;
  lh: number;
  r: number;
  rh: number;
}

/** Each side's lines and the changed blocks between them, as `DiffViewer.vue` lays them out. */
const SIDE = (() => {
  const left: SideLine[] = [];
  const right: SideLine[] = [];
  const blocks: SideBlock[] = [];
  let i = 0;
  while (i < ROWS.length) {
    if ((ROWS[i] as Row).kind === "context") {
      left.push({ index: i, modified: false });
      right.push({ index: i, modified: false });
      i += 1;
      continue;
    }
    const start = i;
    while (i < ROWS.length && (ROWS[i] as Row).kind !== "context") i += 1;
    const indices = Array.from({ length: i - start }, (_, k) => start + k);
    const dels = indices.filter((k) => (ROWS[k] as Row).kind === "del");
    const adds = indices.filter((k) => (ROWS[k] as Row).kind === "add");
    const modified = dels.length > 0 && adds.length > 0;
    blocks.push({ l: left.length + 1, lh: dels.length, r: right.length + 1, rh: adds.length });
    left.push(...dels.map((index) => ({ index, modified })));
    right.push(...adds.map((index) => ({ index, modified })));
  }
  return { left, right, blocks };
})();

const rowPx = computed(() => diffRowPx(font.value));
const sideRows = Math.max(SIDE.left.length, SIDE.right.length) + 1;
const gradientId = `diff-preview-${useId()}`;

/** The bands in the gap, in pixels from the top of the rows, drawn as `DiffViewer.vue` draws them. */
const curves = computed(() => {
  const px = rowPx.value;
  const out: { key: number; d: string; edges: string; kind: "add" | "del" | "mod" | "hunk"; outline: boolean }[] = [];
  out.push({ key: -1, d: bandPath(GAP_PX, 0, px, 0, px), edges: "", kind: "hunk", outline: false });
  SIDE.blocks.forEach((b, i) => {
    const outline = b.lh === 0 || b.rh === 0;
    const [lt, lb] = sideFill(b.l * px, b.lh * px, outline);
    const [rt, rb] = sideFill(b.r * px, b.rh * px, outline);
    const [elt, elb] = sideEdges(b.l * px, b.lh * px);
    const [ert, erb] = sideEdges(b.r * px, b.rh * px);
    const kind = !outline ? "mod" : b.lh === 0 ? "add" : "del";
    out.push({ key: i, d: bandPath(GAP_PX, lt, lb, rt, rb), edges: bandEdges(GAP_PX, elt, elb, ert, erb), kind, outline });
  });
  return out;
});

/** Where an insertion or a deletion has no lines on one side: a line across that side where they would go. */
const insertMarks = computed(() =>
  SIDE.blocks.flatMap((b): { side: "left" | "right"; y: number }[] =>
    b.lh === 0 ? [{ side: "left", y: b.l * rowPx.value }] : b.rh === 0 ? [{ side: "right", y: b.r * rowPx.value }] : [],
  ),
);

const sideColumns = computed(() => [
  { side: "left" as const, lines: SIDE.left, change: "del" as const },
  { side: "right" as const, lines: SIDE.right, change: "add" as const },
]);

// ---- Colours and text --------------------------------------------------------------------------

const settings = useSettingsStore();
const preferredDark = usePreferredDark();
/** The theme chosen in the form, saved or not, which can differ from the window's own. */
const dark = computed(() => {
  const theme = props.values["appearance.theme"] ?? settings.theme;
  return theme === "dark" || (theme === "system" && preferredDark.value);
});
const syntaxHighlight = computed(() => props.values["diff.syntaxHighlight"] === true);
/** The syntax theme the form has chosen for the current app theme. */
const syntaxTheme = computed(() =>
  themeFor(dark.value, { syntaxThemeDark: String(props.values["diff.syntaxThemeDark"] ?? ""), syntaxThemeLight: String(props.values["diff.syntaxThemeLight"] ?? "") }),
);
/** The diff font, and the current theme's colours as the form holds them. */
const style = computed(() => ({ ...diffTextStyle(font.value), ...diffColorVars((setting) => props.values[`diff.${setting}`], dark.value) }));

/** Each row's syntax tokens, by its index in `ROWS`; empty while colouring is off or loading. */
const rowTokens = shallowRef<(RawToken[] | undefined)[]>([]);

watch(
  [syntaxHighlight, syntaxTheme],
  async () => {
    rowTokens.value = [];
    if (!syntaxHighlight.value) return;
    const newSide = ROWS.flatMap((row, i) => (row.kind === "del" ? [] : [i]));
    const oldSide = ROWS.flatMap((row, i) => (row.kind === "add" ? [] : [i]));
    const theme = syntaxTheme.value;
    try {
      const tokens = await highlightRuns("preview.js", theme, [newSide, oldSide].map((side) => side.map((i) => (ROWS[i] as Row).text)));
      if (!tokens || !syntaxHighlight.value || syntaxTheme.value !== theme) return;
      const byRow: (RawToken[] | undefined)[] = [];
      [newSide, oldSide].forEach((side, r) => side.forEach((rowIndex, i) => (byRow[rowIndex] ??= tokens[r]?.[i])));
      rowTokens.value = byRow;
    } catch {
      // The preview stays plain if the language fails to load.
    }
  },
  { immediate: true },
);

/**
 * A row's text as the viewer draws it: syntax colours, with each run of changed words marked
 * once. `gaps` adds the cursor where words were removed, for the side-by-side layout's new side.
 */
function runs(index: number, gaps = false): SpanRun[] {
  const row = ROWS[index] as Row;
  const pair = highlighted(row);
  const segments: Segment[] = pair ? (row.kind === "del" ? pair.old : pair.new) : [{ text: row.text, changed: false }];
  const spans = mergeSpans(rowTokens.value[index], segments);
  return spanRuns(gaps && pair ? insertGaps(spans, pair.removedAt) : spans);
}

function highlighted(row: Row): HighlightedPair | undefined {
  if (!wordHighlight.value || row.other === undefined) return undefined;
  return row.kind === "del" ? highlightPair(row.text, row.other) : highlightPair(row.other, row.text);
}

/** The class a `diff-line` row adds for its kind, as `lineClass` in `DiffViewer.vue` gives it. */
const LINE_CLASS: Record<Kind, string> = {
  context: "",
  add: "diff-line-add",
  del: "diff-line-del",
};
const MARKER: Record<Kind, string> = { context: " ", add: "+", del: "-" };

// ---- Height ------------------------------------------------------------------------------------

/** Drags the handle under the preview to make it taller or shorter. */
function startResize(event: PointerEvent): void {
  if (event.button !== 0) return;
  event.preventDefault();
  const startY = event.clientY;
  const startHeight = height.value;
  const onMove = (e: PointerEvent): void => {
    height.value = Math.round(Math.min(MAX_HEIGHT, Math.max(MIN_HEIGHT, startHeight + e.clientY - startY)));
  };
  const onUp = (e: PointerEvent): void => {
    onMove(e);
    window.removeEventListener("pointermove", onMove);
    window.removeEventListener("pointerup", onUp);
    window.removeEventListener("pointercancel", onUp);
  };
  // On the window, so the drag carries on if the pointer outruns the thin handle.
  window.addEventListener("pointermove", onMove);
  window.addEventListener("pointerup", onUp);
  window.addEventListener("pointercancel", onUp);
}

function onResizeKey(event: KeyboardEvent): void {
  const step = event.key === "ArrowDown" ? 16 : event.key === "ArrowUp" ? -16 : 0;
  if (step === 0) return;
  event.preventDefault();
  height.value = Math.min(MAX_HEIGHT, Math.max(MIN_HEIGHT, height.value + step));
}
</script>

<template>
  <div>
    <div class="mb-1.5 flex justify-end">
      <div class="inline-flex overflow-hidden rounded-md border border-border-soft text-[0.7rem]">
        <button type="button" :aria-pressed="layout === 'unified'" class="toggle h-[18px] rounded-none border-0 px-2 leading-none font-medium" @click="layout = 'unified'">
          Unified
        </button>
        <button
          type="button"
          :aria-pressed="layout === 'side'"
          class="toggle h-[18px] rounded-none border-0 border-l border-l-border-soft px-2 leading-none font-medium"
          @click="layout = 'side'"
        >
          Side by side
        </button>
      </div>
    </div>

    <!-- A fixed height, set by the handle below, so changing the font size or line height never
         moves the fields below; a large font scrolls here instead. -->
    <div
      :class="['area overflow-auto border border-border py-1 whitespace-pre select-none scrollbar-thin', dark ? 'dark' : 'light']"
      :style="[style, { height: `${height}px` }]"
      aria-hidden="true"
    >
      <template v-if="layout === 'unified'">
        <div class="diff-hunk w-max min-w-full"><span class="diff-glyphs">{{ HUNK }}</span></div>
        <div v-for="(row, i) in ROWS" :key="i" :class="['diff-line w-max min-w-full', LINE_CLASS[row.kind]]">
          <span class="diff-num"><span class="diff-glyphs">{{ row.oldNo ?? "" }}</span></span>
          <span class="diff-num"><span class="diff-glyphs">{{ row.newNo ?? "" }}</span></span>
          <span class="diff-sign">{{ MARKER[row.kind] }}</span>
          <span class="diff-code">
            <template v-for="(run, j) in runs(i)" :key="j">
              <mark v-if="run.changed" :class="row.kind === 'del' ? 'diff-word-del' : 'diff-word-add'"><span v-for="(seg, k) in run.spans" :key="k" class="diff-glyphs" :style="spanStyle(seg)">{{ seg.text }}</span></mark>
              <template v-else><span v-for="(seg, k) in run.spans" :key="k" class="diff-glyphs" :style="spanStyle(seg)">{{ seg.text }}</span></template>
            </template>
          </span>
        </div>
      </template>

      <!-- Side by side: each side's own lines from the top, as the viewer draws them scrolled to
           the start, with the bands in the gap between. -->
      <div
        v-else
        class="relative grid"
        :style="{ gridTemplateColumns: `minmax(0, 1fr) ${GAP_PX}px minmax(0, 1fr)`, height: `${sideRows * rowPx}px` }"
      >
        <template v-for="col in sideColumns" :key="col.side">
          <div :class="['relative min-w-0', col.side === 'right' && 'col-start-3']">
            <div class="diff-hunk overflow-hidden"><span class="diff-glyphs">{{ HUNK }}</span></div>
            <div
              v-for="line in col.lines"
              :key="line.index"
              :class="['diff-line min-w-0 overflow-hidden', ROWS[line.index]!.kind === 'context' ? '' : line.modified && !(col.side === 'left' && modLeftAsRemoved) ? 'diff-line-mod' : LINE_CLASS[col.change]]"
            >
              <span class="diff-num"><span class="diff-glyphs">{{ (col.side === 'left' ? ROWS[line.index]!.oldNo : ROWS[line.index]!.newNo) ?? "" }}</span></span>
              <span class="diff-sign">{{ MARKER[ROWS[line.index]!.kind] }}</span>
              <span class="diff-side-code"><span>
                <template v-for="(run, j) in runs(line.index, col.side === 'right')" :key="j">
                  <mark v-if="run.changed" :class="col.side === 'left' ? 'diff-word-del' : 'diff-word-add'"><span v-for="(seg, k) in run.spans" :key="k" :class="glyphClass(seg)" :style="spanStyle(seg)">{{ seg.text }}</span></mark>
                  <template v-else><span v-for="(seg, k) in run.spans" :key="k" :class="glyphClass(seg)" :style="spanStyle(seg)">{{ seg.text }}</span></template>
                </template>
              </span></span>
            </div>
            <div
              v-for="m in insertMarks.filter((mark) => mark.side === col.side)"
              :key="m.y"
              class="absolute inset-x-0 h-px"
              :style="{ top: `${m.y}px`, background: col.side === 'left' ? 'var(--diff-add-mark)' : 'var(--diff-del-mark)' }"
            />
          </div>
        </template>
        <svg class="col-start-2 row-start-1 overflow-visible" :width="GAP_PX" :height="sideRows * rowPx">
          <defs>
            <linearGradient :id="`${gradientId}-mod`" gradientUnits="userSpaceOnUse" x1="0" :x2="GAP_PX" y1="0" y2="0">
              <stop offset="0" class="diff-curve-stop-del" />
              <stop offset="1" class="diff-curve-stop-mod" />
            </linearGradient>
            <linearGradient :id="`${gradientId}-edge-add`" gradientUnits="userSpaceOnUse" x1="0" :x2="GAP_PX" y1="0" y2="0">
              <stop offset="0" class="diff-curve-edge-stop-add" />
              <stop offset="1" class="diff-curve-edge-stop-add diff-curve-edge-stop-clear" />
            </linearGradient>
            <linearGradient :id="`${gradientId}-edge-del`" gradientUnits="userSpaceOnUse" x1="0" :x2="GAP_PX" y1="0" y2="0">
              <stop offset="0" class="diff-curve-edge-stop-del diff-curve-edge-stop-clear" />
              <stop offset="1" class="diff-curve-edge-stop-del" />
            </linearGradient>
          </defs>
          <template v-for="c in curves" :key="c.key">
            <path :d="c.d" :class="`diff-curve-${c.kind}`" :style="c.kind === 'mod' && modLeftAsRemoved ? { fill: `url(#${gradientId}-mod)` } : undefined" />
            <path v-if="c.outline" :d="c.edges" class="diff-curve-edge" :style="{ stroke: `url(#${gradientId}-edge-${c.kind})` }" />
          </template>
        </svg>
      </div>
    </div>

    <!-- Drag to make the preview taller or shorter; the height is kept until the app closes. -->
    <div
      role="separator"
      aria-orientation="horizontal"
      aria-label="Preview height"
      :aria-valuenow="height"
      :aria-valuemin="MIN_HEIGHT"
      :aria-valuemax="MAX_HEIGHT"
      tabindex="0"
      class="group flex h-3 cursor-ns-resize items-center justify-center outline-none"
      @pointerdown="startResize"
      @keydown="onResizeKey"
    >
      <div class="h-1 w-10 rounded-full bg-border group-hover:bg-muted-foreground group-focus-visible:bg-ring" />
    </div>
  </div>
</template>
