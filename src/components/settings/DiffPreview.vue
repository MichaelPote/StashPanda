<script setup lang="ts">
import { computed } from "vue";
import { diffTextStyle, resolveDiffFont } from "@/lib/diff/font";
import { highlightPair, type Segment } from "@/lib/diff/parseUnified";
import { cn } from "@/lib/utils";
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

function segments(row: Row): Segment[] {
  if (!wordHighlight.value || row.other === undefined) return [{ text: row.text, changed: false }];
  const pair = row.kind === "del" ? highlightPair(row.text, row.other) : highlightPair(row.other, row.text);
  return row.kind === "del" ? pair.old : pair.new;
}

const LINE_CLASS: Record<Kind, string> = {
  context: "border-l-2 border-transparent",
  add: "border-l-2 border-transparent bg-diff-add",
  del: "border-l-2 border-transparent bg-diff-del",
};
const MARKER: Record<Kind, string> = { context: " ", add: "+", del: "-" };
</script>

<template>
  <div>
    <!-- A fixed height, so changing the font size or line height never moves the fields below;
         a large font scrolls here instead. The default 12px at 1.2 fits all ten rows. -->
    <div class="area h-44 overflow-auto border border-border py-1 whitespace-pre select-none scrollbar-thin" :style="style" aria-hidden="true">
      <div class="flex h-(--diff-row-h) w-max min-w-full items-center bg-diff-hunk px-2 text-diff-hunk-fg">{{ HUNK }}</div>
      <div v-for="(row, i) in ROWS" :key="i" :class="cn('flex h-(--diff-row-h) w-max min-w-full', LINE_CLASS[row.kind])">
        <span class="w-(--diff-num-w) shrink-0 bg-diff-gutter pr-1 text-right text-gutter-fg tabular-nums shadow-[inset_1px_0_0_rgba(0,0,0,0.25)]">{{ row.oldNo ?? "" }}</span>
        <span class="w-(--diff-num-w) shrink-0 bg-diff-gutter pr-1 text-right text-gutter-fg tabular-nums shadow-[inset_1px_0_0_rgba(0,0,0,0.25)]">{{ row.newNo ?? "" }}</span>
        <span class="w-(--diff-mark-w) shrink-0 text-center text-muted-foreground">{{ MARKER[row.kind] }}</span>
        <span class="flex-1 pr-2">
          <template v-for="(seg, j) in segments(row)" :key="j">
            <mark v-if="seg.changed" :class="row.kind === 'del' ? 'rounded-sm bg-diff-del-mark text-inherit' : 'rounded-sm bg-diff-add-mark text-inherit'">{{ seg.text }}</mark>
            <template v-else>{{ seg.text }}</template>
          </template>
        </span>
      </div>
    </div>
  </div>
</template>
