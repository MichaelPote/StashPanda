<script setup lang="ts">
import { computed, shallowRef, watch } from "vue";
import { usePreferredDark } from "@vueuse/core";
import type { FontSmoothing } from "@/lib/bindings";
import { diffTextStyle, resolveDiffFont } from "@/lib/diff/font";
import { highlightPair, type Segment } from "@/lib/diff/parseUnified";
import { highlightRuns, mergeSpans, spanRuns, spanStyle, themeFor, type SpanRun } from "@/lib/diff/syntax";
import type { RawToken } from "@/lib/diff/syntax.worker";
import { useSettingsStore } from "@/stores/settings";
import type { FormValue } from "./sections";

/**
 * A few diff rows drawn the way `DiffViewer.vue` draws unified rows, using the Diff page's values
 * before they are saved. A value that cannot be saved yet falls back to its default here.
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
const style = computed(() => diffTextStyle(font.value));

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
const OLD_MAP = "\t\treturn names.map((n) => `${greeting}, ${n}`);";
const NEW_MAP = "\t\treturn names.map((n) => `${greeting}, ${n}!`);";

const HUNK = "@@ -12,7 +12,7 @@ export function greet(names) {";
const ROWS: Row[] = [
  { kind: "context", oldNo: 12, newNo: 12, text: "export function greet(names) {" },
  { kind: "del", oldNo: 13, newNo: null, text: OLD_GREETING, other: NEW_GREETING },
  { kind: "add", oldNo: null, newNo: 13, text: NEW_GREETING, other: OLD_GREETING },
  { kind: "context", oldNo: 14, newNo: 14, text: "\tif (names !== undefined && names.length >= 1) {" },
  { kind: "del", oldNo: 15, newNo: null, text: OLD_MAP, other: NEW_MAP },
  { kind: "add", oldNo: null, newNo: 15, text: NEW_MAP, other: OLD_MAP },
  { kind: "context", oldNo: 16, newNo: 16, text: "\t}" },
  { kind: "context", oldNo: 17, newNo: 17, text: '\treturn greeting + " -> nobody";' },
  { kind: "context", oldNo: 18, newNo: 18, text: "}" },
];

const settings = useSettingsStore();
const preferredDark = usePreferredDark();
const dark = computed(() => settings.theme === "dark" || (settings.theme === "system" && preferredDark.value));
const syntaxHighlight = computed(() => props.values["diff.syntaxHighlight"] === true);

/** Each row's syntax tokens, by its index in `ROWS`; empty while colouring is off or loading. */
const rowTokens = shallowRef<(RawToken[] | undefined)[]>([]);

watch(
  [syntaxHighlight, dark],
  async () => {
    rowTokens.value = [];
    if (!syntaxHighlight.value) return;
    const newSide = ROWS.flatMap((row, i) => (row.kind === "del" ? [] : [i]));
    const oldSide = ROWS.flatMap((row, i) => (row.kind === "add" ? [] : [i]));
    const theme = themeFor(dark.value);
    try {
      const tokens = await highlightRuns("preview.js", theme, [newSide, oldSide].map((side) => side.map((i) => (ROWS[i] as Row).text)));
      if (!tokens || !syntaxHighlight.value || themeFor(dark.value) !== theme) return;
      const byRow: (RawToken[] | undefined)[] = [];
      [newSide, oldSide].forEach((side, r) => side.forEach((rowIndex, i) => (byRow[rowIndex] ??= tokens[r]?.[i])));
      rowTokens.value = byRow;
    } catch {
      // The preview stays plain if the language fails to load.
    }
  },
  { immediate: true },
);

/** A row's text as the viewer draws it: syntax colours, with each run of changed words marked once. */
function runs(row: Row, index: number): SpanRun[] {
  return spanRuns(mergeSpans(rowTokens.value[index], segments(row)));
}

function segments(row: Row): Segment[] {
  if (!wordHighlight.value || row.other === undefined) return [{ text: row.text, changed: false }];
  const pair = row.kind === "del" ? highlightPair(row.text, row.other) : highlightPair(row.other, row.text);
  return row.kind === "del" ? pair.old : pair.new;
}

/** The class a `diff-line` row adds for its kind, as `lineClass` in `DiffViewer.vue` gives it. */
const LINE_CLASS: Record<Kind, string> = {
  context: "",
  add: "diff-line-add",
  del: "diff-line-del",
};
const MARKER: Record<Kind, string> = { context: " ", add: "+", del: "-" };
</script>

<template>
  <div>
    <!-- A fixed height, so changing the font size or line height never moves the fields below;
         a large font scrolls here instead. The default 12px at 1.2 fits all ten rows. -->
    <div class="area h-44 overflow-auto border border-border py-1 whitespace-pre select-none scrollbar-thin" :style="style" aria-hidden="true">
      <div class="diff-hunk w-max min-w-full"><span class="diff-glyphs">{{ HUNK }}</span></div>
      <div v-for="(row, i) in ROWS" :key="i" :class="['diff-line w-max min-w-full', LINE_CLASS[row.kind]]">
        <span class="diff-num"><span class="diff-glyphs">{{ row.oldNo ?? "" }}</span></span>
        <span class="diff-num"><span class="diff-glyphs">{{ row.newNo ?? "" }}</span></span>
        <span class="diff-sign">{{ MARKER[row.kind] }}</span>
        <span class="diff-code">
          <template v-for="(run, j) in runs(row, i)" :key="j">
            <mark v-if="run.changed" :class="row.kind === 'del' ? 'diff-word-del' : 'diff-word-add'"><span v-for="(seg, k) in run.spans" :key="k" class="diff-glyphs" :style="spanStyle(seg)">{{ seg.text }}</span></mark>
            <template v-else><span v-for="(seg, k) in run.spans" :key="k" class="diff-glyphs" :style="spanStyle(seg)">{{ seg.text }}</span></template>
          </template>
        </span>
      </div>
    </div>
  </div>
</template>
