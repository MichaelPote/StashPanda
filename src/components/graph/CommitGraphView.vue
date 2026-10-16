<script setup lang="ts">
import { computed, provide, ref, watch } from "vue";
import { useElementSize, useEventListener } from "@vueuse/core";
import type { DateFormat, GraphRow } from "@/lib/bindings";
import PanelHeader from "@/components/shell/PanelHeader.vue";
import EmptyState from "@/components/shell/EmptyState.vue";
import CommitContextMenu from "@/components/graph/CommitContextMenu.vue";
import CommitRow from "@/components/graph/CommitRow.vue";
import GraphFindBar from "@/components/graph/GraphFindBar.vue";
import GraphHeader, { type ResizableColumn } from "@/components/graph/GraphHeader.vue";
import GraphLanesSvg from "@/components/graph/GraphLanesSvg.vue";
import RowSkeleton from "@/components/graph/RowSkeleton.vue";
import VirtualRowList from "@/components/graph/VirtualRowList.vue";
import WorkingTreeContextMenu from "@/components/graph/WorkingTreeContextMenu.vue";
import WorkingTreeRow from "@/components/graph/WorkingTreeRow.vue";
import { GRAPH_CONTEXT } from "@/components/graph/context";
import { useGraphPages } from "@/composables/useGraphPages";
import { autoAuthorWidth, autoDateWidth, autoIdWidth, fontsVersion } from "@/lib/graph/columnWidths";
import { autoGraphColumnWidth, messageIndent, ROW_HEIGHT } from "@/lib/graph/paths";
import { isEditableTarget } from "@/lib/files";
import { useSessionsStore, type RepoSession } from "@/stores/sessions";
import { useDialogsStore } from "@/stores/dialogs";
import { useSettingsStore } from "@/stores/settings";
import { useUiStateStore } from "@/stores/uiState";

const props = defineProps<{
  session: RepoSession;
}>();

const sessions = useSessionsStore();
const settings = useSettingsStore();
const uiState = useUiStateStore();
const pages = useGraphPages(props.session);

/** Rows in one Shift-range at most, so a range over unloaded pages stays bounded. */
const MAX_RANGE = 10_000;

// ---- Context for rows ----------------------------------------------------------------------

const dateFormat = computed<DateFormat>(() => settings.settings?.appearance?.dateFormat ?? "relative");
const upstreamRemotes = computed(() => {
  const map = new Map<string, string>();
  const refs = props.session.refs?.refs ?? [];
  for (const entry of refs) {
    if (entry.kind.type !== "localBranch" || !entry.upstream || entry.upstreamGone) continue;
    // The remote as the refs list gives it, since a remote's name may contain "/" (`team/origin`).
    const upstream = refs.find((r) => r.fullName === entry.upstream);
    if (upstream?.kind.type === "remoteBranch") {
      map.set(entry.fullName, upstream.kind.remote);
      continue;
    }
    const rest = entry.upstream.startsWith("refs/remotes/") ? entry.upstream.slice("refs/remotes/".length) : entry.upstream;
    const slash = rest.indexOf("/");
    map.set(entry.fullName, slash > 0 ? rest.slice(0, slash) : rest);
  }
  return map;
});
const trackedRemotes = computed(() => {
  const set = new Set<string>();
  for (const entry of props.session.refs?.refs ?? []) {
    if (entry.kind.type === "localBranch" && entry.upstream && !entry.upstreamGone) set.add(entry.upstream);
  }
  return set;
});
provide(GRAPH_CONTEXT, { sessionKey: props.session.key, dateFormat, upstreamRemotes, trackedRemotes });

// ---- Columns -------------------------------------------------------------------------------

const meta = computed(() => props.session.graph.meta);
const total = computed(() => meta.value?.total ?? 0);
// Null (or absent) means "auto": follow the lane count of the current snapshot. A drag on the
// header handle sets a width, which lasts until the lane count changes (commits load, or refs
// are hidden or shown in the Branches panel); double-clicking the handle returns to auto at once.
const graphWidth = computed(
  () => uiState.state?.graphColumns?.graph ?? autoGraphColumnWidth(meta.value?.maxLanes ?? 0),
);
/**
 * With `appearance.graphInMessageColumn` the lanes are drawn over the start of the commit
 * message column, which then takes the graph column's place, and each row's message starts just
 * past the lanes on that row (`messageIndent`).
 */
const graphInMessages = computed(() => settings.settings?.appearance?.graphInMessageColumn ?? false);
/** How wide the lanes are drawn: the graph column, or every lane when they share the message column. */
const lanesWidth = computed(() => (graphInMessages.value ? autoGraphColumnWidth(meta.value?.maxLanes ?? 0) : graphWidth.value));
watch(
  () => meta.value?.maxLanes,
  (lanes, before) => {
    if (lanes === undefined || before === undefined || lanes === before) return;
    if (uiState.state?.graphColumns?.graph != null) onResize("graph", null);
  },
);
// Author, Date and ID size themselves to their widest possible value (re-measured once the web
// fonts load) until a divider is dragged; double-clicking a divider returns to automatic.
const authorWidth = computed(() => {
  void fontsVersion.value;
  return uiState.state?.graphColumns?.author ?? autoAuthorWidth(meta.value?.widestAuthors ?? []);
});
const dateWidth = computed(() => {
  void fontsVersion.value;
  return uiState.state?.graphColumns?.date ?? autoDateWidth(dateFormat.value);
});
const idWidth = computed(() => {
  void fontsVersion.value;
  return uiState.state?.graphColumns?.id ?? autoIdWidth();
});

const columnStyle = computed(() => ({
  "--col-graph": graphInMessages.value ? "0px" : `${graphWidth.value}px`,
  "--col-author": `${authorWidth.value}px`,
  "--col-date": `${dateWidth.value}px`,
  "--col-id": `${idWidth.value}px`,
}));

function onResize(column: ResizableColumn, width: number | null): void {
  // `patchDebounced` applies the value locally at once and writes to disk after 300 ms.
  // A null width removes the key (merge patch), which the backend stores as "auto".
  uiState.patchDebounced({ graphColumns: { [column]: width } });
}

// ---- Rows in the window --------------------------------------------------------------------

const list = ref<InstanceType<typeof VirtualRowList> | null>(null);
/** The rows' width inside the scroller, without its scrollbar. */
const { width: rowsWidth } = useElementSize(computed(() => list.value?.element ?? null));
/**
 * How wide the lanes are drawn. Sharing the message column, they stop at its right edge, so a
 * message column narrowed by wide Author, Date or ID columns clips the outer lanes rather than
 * drawing them over those columns.
 */
const lanesClip = computed(() => {
  if (!graphInMessages.value || rowsWidth.value <= 0) return lanesWidth.value;
  return Math.max(0, Math.min(lanesWidth.value, rowsWidth.value - authorWidth.value - dateWidth.value - idWidth.value));
});
const start = computed(() => list.value?.start ?? 0);
const end = computed(() => list.value?.end ?? 0);

/** Where row `index`'s message starts when the graph shares its column, or null when it has a column of its own. */
function indentOf(index: number, row: GraphRow): number | null {
  return graphInMessages.value ? messageIndent(row, index > 0 ? pages.rowAt(index - 1) : undefined) : null;
}

const visibleRows = computed<(GraphRow | undefined)[]>(() => {
  // Reading `pagesVersion` makes this recompute when a page lands.
  void props.session.graph.pagesVersion.value;
  const s = start.value;
  const e = end.value;
  const out: (GraphRow | undefined)[] = [];
  for (let i = s; i < e; i += 1) out.push(pages.rowAt(i));
  return out;
});

/** One extra row above the block so an edge from row start-1 still curves into row start. */
const svgFirst = computed(() => Math.max(0, start.value - 1));
const svgRows = computed<(GraphRow | undefined)[]>(() => {
  void props.session.graph.pagesVersion.value;
  const out: (GraphRow | undefined)[] = [];
  for (let i = svgFirst.value; i < end.value; i += 1) out.push(pages.rowAt(i));
  return out;
});

watch(
  () => [start.value, end.value, meta.value?.version ?? 0, props.session.graph.pagesVersion.value] as const,
  ([s, e]) => {
    void pages.ensureRange(Math.max(0, s - 1), e);
  },
  { immediate: true },
);

// Pages far from the window are dropped as it moves, so a long scroll does not keep them all.
watch(
  () => [start.value, end.value] as const,
  ([s, e]) => pages.dropFarPages(Math.max(0, s - 1), e),
);


// ---- Selection -----------------------------------------------------------------------------

const selection = computed(() => props.session.selection);
/** Snapshot index the keyboard moves from; re-derived from the anchor oid after a snapshot swap. */
const cursorIndex = ref<number | null>(null);

function currentIndex(): number | null {
  const anchor = selection.value.anchorOid;
  if (anchor === null) return cursorIndex.value;
  const at = cursorIndex.value;
  if (at !== null && pages.rowAt(at)?.oid === anchor) return at;
  return pages.findLoadedIndex(anchor) ?? cursorIndex.value;
}

function selectSingle(row: GraphRow): void {
  cursorIndex.value = row.index;
  sessions.selectRow(props.session, row);
}

function toggle(row: GraphRow): void {
  const oids = new Set(selection.value.oids);
  if (oids.has(row.oid)) {
    oids.delete(row.oid);
    props.session.selection = { oids, anchorOid: row.oid };
    if (props.session.selectedRow?.oid === row.oid) {
      props.session.selectedRow = null;
      props.session.selectedFile = null;
    }
  } else {
    oids.add(row.oid);
    props.session.selection = { oids, anchorOid: row.oid };
    props.session.selectedRow = row;
    props.session.selectedFile = null;
  }
  cursorIndex.value = row.index;
}

async function selectRangeTo(index: number): Promise<void> {
  const from = currentIndex() ?? index;
  let lo = Math.min(from, index);
  let hi = Math.max(from, index);
  if (hi - lo + 1 > MAX_RANGE) {
    if (index > from) hi = lo + MAX_RANGE - 1;
    else lo = hi - MAX_RANGE + 1;
  }
  await pages.ensureRange(lo, hi + 1);
  const oids = new Set<string>();
  for (let i = lo; i <= hi; i += 1) {
    const r = pages.rowAt(i);
    if (r) oids.add(r.oid);
  }
  const target = pages.rowAt(index);
  props.session.selection = { oids, anchorOid: selection.value.anchorOid ?? target?.oid ?? null };
  if (target) {
    props.session.selectedRow = target;
    props.session.selectedFile = null;
  }
  cursorIndex.value = index;
}

/** Shift+click selects a range of rows, so it must not also extend a text selection. */
function onMouseDown(event: MouseEvent): void {
  if (event.shiftKey) event.preventDefault();
}

function onClick(event: MouseEvent): void {
  const target = event.target as HTMLElement | null;
  const rowEl = target?.closest<HTMLElement>("[data-index]");
  if (!rowEl) return;
  const index = Number(rowEl.dataset.index);
  const row = pages.rowAt(index);
  if (!row) return;
  if (event.shiftKey) void selectRangeTo(index);
  else if (event.ctrlKey || event.metaKey) toggle(row);
  else selectSingle(row);
  // Ref pills are buttons and keep focus themselves (a popover may be opening).
  if (!target?.closest("button")) container.value?.focus();
}

/**
 * A right-click selects the row before its context menu opens, unless the row is already part
 * of the selection. Ref pills stop the event, so a right-click on a pill leaves the selection alone.
 */
function onContextMenu(event: MouseEvent): void {
  const target = event.target as HTMLElement | null;
  const rowEl = target?.closest<HTMLElement>("[data-index]");
  if (!rowEl) return;
  const row = pages.rowAt(Number(rowEl.dataset.index));
  if (!row || row.kind !== "commit") return;
  if (!selection.value.oids.has(row.oid)) selectSingle(row);
}

const container = ref<HTMLElement | null>(null);

async function moveTo(index: number, extend: boolean): Promise<void> {
  const n = total.value;
  if (n === 0) return;
  const clamped = Math.min(n - 1, Math.max(0, index));
  list.value?.ensureVisible(clamped);
  await pages.ensureRange(clamped, clamped + 1);
  const row = pages.rowAt(clamped);
  if (!row) return;
  if (extend) await selectRangeTo(clamped);
  else selectSingle(row);
}

async function copySelection(): Promise<void> {
  const oids = [...selection.value.oids].filter((o) => o !== "WORKTREE");
  if (oids.length === 0) return;
  try {
    await navigator.clipboard.writeText(oids.join("\n"));
  } catch (e) {
    sessions.notify(`Could not copy to the clipboard: ${String(e)}`);
  }
}

function onKeydown(event: KeyboardEvent): void {
  if ((event.ctrlKey || event.metaKey) && (event.key === "c" || event.key === "C")) {
    event.preventDefault();
    void copySelection();
    return;
  }
  if (event.key === "Escape" && props.session.findOpen) {
    event.preventDefault();
    props.session.findOpen = false;
    return;
  }
  const n = total.value;
  if (n === 0) return;
  const cur = currentIndex();
  const page = list.value?.rowsPerPage() ?? 20;
  let next: number;
  switch (event.key) {
    case "ArrowDown":
      next = cur === null ? 0 : cur + 1;
      break;
    case "ArrowUp":
      next = cur === null ? 0 : cur - 1;
      break;
    case "PageDown":
      next = (cur ?? 0) + page;
      break;
    case "PageUp":
      next = (cur ?? 0) - page;
      break;
    case "Home":
      next = 0;
      break;
    case "End":
      next = n - 1;
      break;
    default:
      return;
  }
  event.preventDefault();
  void moveTo(next, event.shiftKey);
}

// A ref click in the Branches panel asks for a row to be scrolled into view.
watch(
  () => props.session.reveal,
  (reveal) => {
    if (!reveal) return;
    cursorIndex.value = reveal.index;
    list.value?.ensureVisible(reveal.index);
    props.session.reveal = null;
  },
);

function onScrollTop(top: number): void {
  props.session.scrollTop = top;
}

// ---- Find box ------------------------------------------------------------------------------

function openFind(): void {
  props.session.findOpen = true;
  props.session.findFocus += 1;
}

// Ctrl+F anywhere in the active tab opens the Find box, except in a text field, where it is
// left to the field, in a diff view, which has a Find box of its own and marks the key handled,
// and while a dialog is open over the window. The native Edit > Find Commit item does the same
// through `edit.find`.
const dialogs = useDialogsStore();
useEventListener(window, "keydown", (event: KeyboardEvent) => {
  if (!(event.ctrlKey || event.metaKey) || event.altKey || event.shiftKey || (event.key !== "f" && event.key !== "F")) return;
  if (sessions.activeKey !== props.session.key || event.defaultPrevented || isEditableTarget(event.target)) return;
  if (dialogs.active !== null) return;
  event.preventDefault();
  openFind();
});

function onFindClosed(): void {
  container.value?.focus();
}
</script>

<template>
  <section ref="container" class="area flex h-full min-h-0 flex-col outline-none" :style="columnStyle" tabindex="0" @keydown="onKeydown">
    <PanelHeader title="Commits">
      <span class="truncate text-xs text-muted-foreground select-text">{{ session.info.toplevel }}</span>
      <template #actions>
        <span v-if="meta && meta.total > 0" class="text-[0.7rem] text-muted-foreground tabular-nums">
          {{ meta.total.toLocaleString() }} rows<template v-if="meta.truncated"> (truncated)</template>
        </span>
      </template>
    </PanelHeader>
    <GraphHeader
      :graph-width="graphWidth"
      :author-width="authorWidth"
      :date-width="dateWidth"
      :id-width="idWidth"
      :graph-column="!graphInMessages"
      @resize="onResize"
    />

    <EmptyState v-if="meta && meta.version > 0 && meta.total === 0 && !session.graph.loading" message="This repository has no commits yet." />

    <div v-else class="relative min-h-0 flex-1" @mousedown="onMouseDown" @click="onClick" @contextmenu="onContextMenu">
      <VirtualRowList
        ref="list"
        :total="total"
        :row-height="ROW_HEIGHT"
        :saved-scroll-top="session.scrollTop"
        @update:scroll-top="onScrollTop"
      >
        <template #default="{ start: s }">
          <div class="graph-lanes-clip pointer-events-none absolute top-0 left-0 z-10 h-full overflow-hidden" :style="{ width: `${lanesClip}px` }">
            <GraphLanesSvg :rows="svgRows" :first="svgFirst" :block-start="s" :row-height="ROW_HEIGHT" :width="lanesWidth" />
          </div>
          <template v-for="(row, k) in visibleRows" :key="s + k">
            <WorkingTreeContextMenu v-if="row && row.kind === 'workingTree'" :session="session">
              <WorkingTreeRow :row="row" :selected="selection.oids.has(row.oid)" :indent="indentOf(s + k, row)" />
            </WorkingTreeContextMenu>
            <CommitContextMenu v-else-if="row" :row="row" :session="session">
              <CommitRow :row="row" :selected="selection.oids.has(row.oid)" :indent="indentOf(s + k, row)" />
            </CommitContextMenu>
            <RowSkeleton v-else :index="s + k" />
          </template>
        </template>
      </VirtualRowList>
      <GraphFindBar
        v-if="session.findOpen"
        class="absolute top-1.5 right-4 z-20"
        :session="session"
        :current="currentIndex"
        @close="onFindClosed"
        @mousedown.stop
        @click.stop
        @contextmenu.stop
      />
    </div>
  </section>
</template>

<style>
.graph-grid {
  display: grid;
  grid-template-columns: var(--col-graph) minmax(0, 1fr) var(--col-author) var(--col-date) var(--col-id);
}
.graph-row {
  contain: strict;
  position: relative;
}
</style>
