<script setup lang="ts">
import { remToPx } from "@/lib/graph/paths";
import { computed, ref, shallowRef, watch } from "vue";
import { useDebounceFn, useLocalStorage } from "@vueuse/core";
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

const props = defineProps<{
  session: RepoSession;
}>();

/** Diff row height: the `--diff-row-h` rem value from index.css, as pixels for the virtual list. */
const ROW_H = remToPx(1.4);
const ALL_CONTEXT = 99999;

type Mode = "unified" | "side";
const mode = useLocalStorage<Mode>("stashpanda.diff.mode", "unified");
const contextLines = useLocalStorage<number>("stashpanda.diff.context", 3);
const ignoreWhitespace = useLocalStorage<boolean>("stashpanda.diff.ignoreWhitespace", false);
const ignoreCrAtEol = useLocalStorage<boolean>("stashpanda.diff.ignoreCrAtEol", false);

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
}

const built = computed<Built>(() => {
  const unified: UnifiedRow[] = [];
  const side: SideRow[] = [];
  const pairs: Pair[] = [];
  let maxLen = 0;
  const f = file.value;
  if (!f) return { unified, side, pairs, maxLen };

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
      side.push({ kind: "pair", hunk: h, left: line, right: line });
    }
    flush();
  });
  return { unified, side, pairs, maxLen };
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
const virtual = useVirtualRows(scroller, { rowHeight: ROW_H, total: () => rowCount.value, overscan: 10 });

const visibleUnified = computed(() => built.value.unified.slice(virtual.start.value, virtual.end.value));
const visibleSide = computed(() => built.value.side.slice(virtual.start.value, virtual.end.value));

/** Gutter (two line numbers plus marker) in pixels, then the widest line in `ch`. */
const GUTTER_PX = 92;
const contentMinWidth = computed(() => {
  const chars = Math.max(20, built.value.maxLen + 2);
  return mode.value === "unified" ? `calc(${GUTTER_PX}px + ${chars}ch)` : `calc(${2 * GUTTER_PX}px + ${2 * chars}ch)`;
});

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
  return Math.floor(virtual.scrollTop.value / ROW_H);
}

function nextHunk(): void {
  const cur = firstVisibleRow();
  const target = hunkRows.value.find((i) => i > cur + 1);
  if (target !== undefined) virtual.scrollTo(target * ROW_H);
}

function prevHunk(): void {
  const cur = firstVisibleRow();
  const before = hunkRows.value.filter((i) => i < cur - 1);
  const target = before[before.length - 1];
  virtual.scrollTo((target ?? 0) * ROW_H);
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
      return "border-l-2 border-added bg-added/15";
    case "del":
      return "border-l-2 border-deleted bg-deleted/15";
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
  <section class="flex h-full min-h-0 flex-col">
    <PanelHeader title="Diff">
      <span v-if="selected" class="truncate font-mono text-xs text-foreground" :title="headerPath">{{ headerPath }}</span>
      <LoaderCircleIcon v-if="loading" class="size-3 shrink-0 animate-spin text-muted-foreground" />
      <template #actions>
        <div v-if="selected" class="flex items-center gap-1 text-[0.7rem]">
          <div class="inline-flex overflow-hidden rounded-full border border-border">
            <button
              type="button"
              :aria-pressed="mode === 'unified'"
              :class="cn('h-[1.2rem] px-2 leading-none', mode === 'unified' ? 'bg-accent text-accent-foreground' : 'text-muted-foreground hover:text-foreground')"
              @click="mode = 'unified'"
            >
              Unified
            </button>
            <button
              type="button"
              :aria-pressed="mode === 'side'"
              :class="cn('h-[1.2rem] border-l border-border px-2 leading-none', mode === 'side' ? 'bg-accent text-accent-foreground' : 'text-muted-foreground hover:text-foreground')"
              @click="mode = 'side'"
            >
              Side by side
            </button>
          </div>
          <label class="inline-flex items-center gap-1 text-muted-foreground">
            Context
            <select
              class="h-[1.2rem] rounded border border-border bg-background px-1 text-[0.7rem] text-foreground"
              :value="String(contextLines)"
              @change="toggleContext(($event.target as HTMLSelectElement).value)"
            >
              <option v-for="c in CONTEXT_CHOICES" :key="c.value" :value="String(c.value)">{{ c.label }}</option>
            </select>
          </label>
          <button
            type="button"
            :aria-pressed="ignoreWhitespace"
            :class="cn('h-[1.2rem] rounded-full border px-2 leading-none', ignoreWhitespace ? 'border-transparent bg-accent text-accent-foreground' : 'border-border text-muted-foreground hover:text-foreground')"
            title="Ignore whitespace changes (git diff -w)"
            @click="ignoreWhitespace = !ignoreWhitespace"
          >
            Ignore whitespace
          </button>
          <button
            type="button"
            :aria-pressed="ignoreCrAtEol"
            :class="cn('h-[1.2rem] rounded-full border px-2 leading-none', ignoreCrAtEol ? 'border-transparent bg-accent text-accent-foreground' : 'border-border text-muted-foreground hover:text-foreground')"
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
    <EmptyState v-else-if="error" :message="error" class="text-deleted-fg" />
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

      <div ref="scroller" class="relative min-h-0 flex-1 overflow-auto font-mono text-xs leading-(--diff-row-h) scrollbar-thin">
        <div class="relative" :style="{ height: `${rowCount * ROW_H}px`, minWidth: contentMinWidth }">
          <div class="absolute inset-x-0 top-0 will-change-transform" :style="{ transform: `translateY(${virtual.start.value * ROW_H}px)` }">
            <!-- Unified -->
            <template v-if="mode === 'unified'">
              <template v-for="(row, k) in visibleUnified" :key="virtual.start.value + k">
                <div v-if="row.kind === 'hunk'" class="flex h-5 items-center bg-renamed/10 px-2 whitespace-pre text-renamed-fg">
                  {{ row.header }}
                </div>
                <div v-else :class="cn('flex h-5 whitespace-pre', lineClass(row.line?.kind))">
                  <span class="w-10 shrink-0 pr-1 text-right text-muted-foreground/70 tabular-nums select-none">{{ row.line?.oldNo ?? "" }}</span>
                  <span class="w-10 shrink-0 pr-1 text-right text-muted-foreground/70 tabular-nums select-none">{{ row.line?.newNo ?? "" }}</span>
                  <span class="w-3 shrink-0 text-center text-muted-foreground select-none">{{ marker(row.line?.kind) }}</span>
                  <span class="flex-1 pr-2">
                    <template v-for="(seg, i) in segments(row.pair, row.line?.kind === 'del' ? 'old' : 'new', row.line?.text ?? '')" :key="i">
                      <mark v-if="seg.changed" :class="row.line?.kind === 'del' ? 'rounded-sm bg-deleted/40 text-inherit' : 'rounded-sm bg-added/40 text-inherit'">{{ seg.text }}</mark>
                      <template v-else>{{ seg.text }}</template>
                    </template>
                  </span>
                </div>
              </template>
            </template>

            <!-- Side by side -->
            <template v-else>
              <template v-for="(row, k) in visibleSide" :key="virtual.start.value + k">
                <div v-if="row.kind === 'hunk'" class="flex h-5 items-center bg-renamed/10 px-2 whitespace-pre text-renamed-fg">
                  {{ row.header }}
                </div>
                <div v-else-if="row.kind === 'meta'" class="flex h-5 items-center border-l-2 border-transparent px-11 whitespace-pre text-muted-foreground italic">
                  \ {{ row.text }}
                </div>
                <div v-else class="flex h-5">
                  <div :class="cn('flex min-w-0 flex-1 whitespace-pre', row.left ? lineClass(row.left.kind === 'context' ? 'context' : 'del') : 'border-l-2 border-transparent bg-muted/20')">
                    <span class="w-10 shrink-0 pr-1 text-right text-muted-foreground/70 tabular-nums select-none">{{ row.left?.oldNo ?? "" }}</span>
                    <span class="w-3 shrink-0 text-center text-muted-foreground select-none">{{ row.left ? marker(row.left.kind === 'context' ? 'context' : 'del') : "" }}</span>
                    <span class="flex-1 pr-2">
                      <template v-if="row.left">
                        <template v-for="(seg, i) in segments(row.pair, 'old', row.left.text)" :key="i">
                          <mark v-if="seg.changed" class="rounded-sm bg-deleted/40 text-inherit">{{ seg.text }}</mark>
                          <template v-else>{{ seg.text }}</template>
                        </template>
                      </template>
                    </span>
                  </div>
                  <div :class="cn('flex min-w-0 flex-1 border-l border-border whitespace-pre', row.right ? lineClass(row.right.kind === 'context' ? 'context' : 'add') : 'border-l-2 border-transparent bg-muted/20')">
                    <span class="w-10 shrink-0 pr-1 text-right text-muted-foreground/70 tabular-nums select-none">{{ row.right?.newNo ?? "" }}</span>
                    <span class="w-3 shrink-0 text-center text-muted-foreground select-none">{{ row.right ? marker(row.right.kind === 'context' ? 'context' : 'add') : "" }}</span>
                    <span class="flex-1 pr-2">
                      <template v-if="row.right">
                        <template v-for="(seg, i) in segments(row.pair, 'new', row.right.text)" :key="i">
                          <mark v-if="seg.changed" class="rounded-sm bg-added/40 text-inherit">{{ seg.text }}</mark>
                          <template v-else>{{ seg.text }}</template>
                        </template>
                      </template>
                    </span>
                  </div>
                </div>
              </template>
            </template>
          </div>
        </div>
      </div>
    </template>
  </section>
</template>
