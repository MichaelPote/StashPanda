<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { useEventListener, useResizeObserver } from "@vueuse/core";
import { GitBranchIcon, GitForkIcon, LoaderCircleIcon } from "@lucide/vue";
import { commands, type DiffTarget, type FileChange, type MergeInfo, type ResolveSide } from "@/lib/bindings";
import PanelHeader from "@/components/shell/PanelHeader.vue";
import EmptyState from "@/components/shell/EmptyState.vue";
import MessageToggle from "@/components/files/MessageToggle.vue";
import ActionMenuItem from "@/components/graph/ActionMenuItem.vue";
import { ContextMenu, ContextMenuContent, ContextMenuSeparator, ContextMenuTrigger } from "@/components/ui/context-menu";
import {
  copyFullPath,
  copyText,
  discardSelected,
  fileActionReason,
  markResolved,
  openFileReason,
  openWorktreeFile,
  revealWorktreeFile,
  selectedConflictItems,
  selectedWorktreePaths,
  stageSelected,
  takeVersion,
  unstageSelected,
} from "@/lib/repoActions";
import { conflictStages, operationNames } from "@/lib/conflicts";
import { formatAbsoluteTime, formatRelativeTime, isoTime, shortOid } from "@/lib/format";
import { errorMessage, unwrap } from "@/lib/ipc";
import {
  STATE_CLASSES,
  isEditableTarget,
  isSelectAllKey,
  letterState,
  splitPath,
  statusLetter,
  arrangeSections,
  worktreeSections as buildWorktreeSections,
  type FileItem,
  type FileSection,
  type GroupBy,
  type SortBy,
} from "@/lib/files";
import { fontsVersion, rowTextWidth } from "@/lib/graph/columnWidths";
import { remToPx } from "@/lib/graph/paths";
import { cn } from "@/lib/utils";
import { mergeParentLabels, mergeParentNames, mergeSummary, type RepoNames } from "@/lib/mergeParents";
import { useListSelection } from "@/composables/useListSelection";
import { useDialogsStore } from "@/stores/dialogs";
import { emptyFileSelection, useSessionsStore, type RepoSession, type SelectedFile } from "@/stores/sessions";
import { useSettingsStore } from "@/stores/settings";
import { useUiStateStore } from "@/stores/uiState";

const props = defineProps<{
  session: RepoSession;
}>();

const settings = useSettingsStore();
const sessions = useSessionsStore();
const dialogs = useDialogsStore();
const uiState = useUiStateStore();

const selectedRow = computed(() => props.session.selectedRow);
const mode = computed<"none" | "commit" | "worktree">(() => {
  const row = selectedRow.value;
  if (!row) return "none";
  return row.kind === "workingTree" ? "worktree" : "commit";
});

// ---- Commit mode ---------------------------------------------------------------------------

const parentIndex = ref(0);
const commitParents = computed(() => (mode.value === "commit" ? (selectedRow.value?.parents ?? []) : []));
const isMerge = computed(() => commitParents.value.length > 1);
/** Branch names for the merge's parents, for the Compare with toggles. */
const parentLabels = computed(() => mergeParentLabels(selectedRow.value?.subject ?? "", graphBranches.value, repoNames.value));

/** For a merge: its graph branch, and each parent's branch and whether the merge kept its files. */
const mergeInfo = ref<MergeInfo | null>(null);
let mergeInfoRequest = 0;

async function loadMergeInfo(): Promise<void> {
  const row = selectedRow.value;
  const request = (mergeInfoRequest += 1);
  mergeInfo.value = null;
  if (!row || row.kind !== "commit" || row.parents.length < 2) return;
  try {
    const info = await unwrap(commands.getMergeInfo(props.session.key, row.oid));
    if (request === mergeInfoRequest) mergeInfo.value = info;
  } catch {
    // Without the answer, treat it as an ordinary merge and offer both parents.
    if (request === mergeInfoRequest) {
      mergeInfo.value = { branch: null, parents: row.parents.map((oid) => ({ oid, unchanged: false, branch: null })) };
    }
  }
}
const unchangedParents = computed(() => mergeInfo.value?.parents.map((p) => p.unchanged) ?? null);
/** The graph's branch for each parent: the merge commit's own for the first, since it was merged into that one. */
const graphBranches = computed(() =>
  commitParents.value.map((oid, i) =>
    i === 0 ? (mergeInfo.value?.branch ?? null) : (mergeInfo.value?.parents.find((p) => p.oid === oid)?.branch ?? null),
  ),
);
/** The repository's branches and remotes, for naming merge parents. */
const repoNames = computed<RepoNames>(() => ({
  branches: (props.session.refs?.refs ?? []).filter((r) => r.kind.type === "localBranch" || r.kind.type === "remoteBranch").map((r) => r.short),
  remotes: props.session.refs?.remotes ?? [],
}));

/**
 * A straight merge kept one parent's files unchanged, as `git merge --no-ff` does onto a branch
 * with no commits of its own, so comparing with that parent lists nothing. This is the index of
 * the other parent, the only one worth comparing with; null for any other commit.
 */
const straightMergeParent = computed<number | null>(() => {
  const unchanged = unchangedParents.value;
  if (!unchanged || unchanged.length !== 2 || unchanged[0] === unchanged[1]) return null;
  return unchanged[0] ? 1 : 0;
});
const straightMergeText = computed(() =>
  mergeSummary(mergeParentNames(selectedRow.value?.subject ?? "", graphBranches.value, repoNames.value)),
);

/** Parent passed to the backend: only set explicitly for merges, `null` means "first parent". */
const parentOid = computed<string | null>(() => {
  if (!isMerge.value) return null;
  const index = straightMergeParent.value ?? parentIndex.value;
  return commitParents.value[index] ?? commitParents.value[0] ?? null;
});

const changes = ref<FileChange[]>([]);
const changesLoading = ref(false);
const changesError = ref<string | null>(null);
let changesRequest = 0;

async function loadChanges(): Promise<void> {
  const row = selectedRow.value;
  if (!row || row.kind !== "commit") {
    changes.value = [];
    return;
  }
  const request = (changesRequest += 1);
  changesLoading.value = true;
  changesError.value = null;
  // A merge waits to learn whether it is a straight merge, which decides the parent to compare with.
  if (isMerge.value && unchangedParents.value === null) {
    changes.value = [];
    return;
  }
  try {
    const list = await unwrap(commands.getCommitChanges(props.session.key, row.oid, parentOid.value));
    if (request !== changesRequest) return;
    changes.value = list;
  } catch (e) {
    if (request !== changesRequest) return;
    changes.value = [];
    changesError.value = errorMessage(e);
  } finally {
    if (request === changesRequest) changesLoading.value = false;
  }
}

const message = ref<string | null>(null);
const messageExpanded = ref(false);
const messageLoading = ref(false);
const messageError = ref<string | null>(null);

watch(
  () => selectedRow.value?.oid,
  () => {
    parentIndex.value = 0;
    props.session.fileSelection = emptyFileSelection();
    message.value = null;
    messageExpanded.value = false;
    messageError.value = null;
  },
);

watch(
  () => selectedRow.value?.oid,
  () => {
    void loadMergeInfo();
  },
  { immediate: true },
);

watch(
  () => [selectedRow.value?.oid, parentOid.value, mode.value, unchangedParents.value] as const,
  () => {
    void loadChanges();
  },
  { immediate: true },
);

/** Everything after the first paragraph of the full commit message. */
const messageBody = computed(() => {
  const full = message.value;
  if (full === null) return "";
  const normalised = full.replace(/\r\n/g, "\n").trimEnd();
  const split = normalised.indexOf("\n\n");
  return split < 0 ? "" : normalised.slice(split + 2);
});

async function toggleMessage(): Promise<void> {
  messageExpanded.value = !messageExpanded.value;
  const row = selectedRow.value;
  if (!messageExpanded.value || message.value !== null || !row || row.kind !== "commit") return;
  messageLoading.value = true;
  try {
    const text = await unwrap(commands.getCommitMessage(props.session.key, row.oid));
    if (selectedRow.value?.oid === row.oid) message.value = text;
  } catch (e) {
    messageError.value = errorMessage(e);
  } finally {
    messageLoading.value = false;
  }
}

const commitTarget = computed<DiffTarget | null>(() => {
  const row = selectedRow.value;
  if (!row || row.kind !== "commit") return null;
  return { type: "commit", commit: row.oid, parent: parentOid.value };
});

const commitSections = computed<FileSection[]>(() => {
  const target = commitTarget.value;
  if (!target) return [];
  const items: FileItem[] = changes.value.map((c) => {
    const letter = statusLetter(c.status);
    return {
      id: `commit:${c.path}`,
      path: c.path,
      oldPath: c.oldPath,
      letter,
      state: letterState(letter),
      added: c.added,
      deleted: c.deleted,
      binary: c.binary,
      target,
    };
  });
  return [{ key: "commit", title: "Changed files", items }];
});

const dateText = computed(() => {
  const row = selectedRow.value;
  if (!row) return "";
  return settings.settings?.appearance?.dateFormat === "absolute"
    ? formatAbsoluteTime(row.authorTime)
    : formatRelativeTime(row.authorTime);
});

// ---- Working tree mode ---------------------------------------------------------------------

const worktreeSections = computed<FileSection[]>(() => buildWorktreeSections(props.session.status));

// ---- Shared list behaviour -----------------------------------------------------------------

// ---- Grouping and sorting ------------------------------------------------------------------

const groupBy = uiState.pref<GroupBy>((s) => s.files?.groupBy, "status", (groupBy) => ({ files: { groupBy } }));
const sortBy = uiState.pref<SortBy>((s) => s.files?.sortBy, "name", (sortBy) => ({ files: { sortBy } }));

/** Sorting by date needs the working tree; a commit's files all share one date. */
const sortingByDate = computed(() => sortBy.value === "modified" && mode.value === "worktree");

/** Last-modified time per path, fetched when sorting by date and after every status refresh. */
const times = ref<Map<string, number | null> | null>(null);
let timesRequest = 0;

watch(
  () => [sortingByDate.value, props.session.status] as const,
  async ([byDate, status]) => {
    const request = (timesRequest += 1);
    if (!byDate || !status) {
      times.value = null;
      return;
    }
    const paths = [...new Set(status.entries.filter((e) => !e.ignored).map((e) => e.path))];
    if (paths.length === 0) {
      times.value = new Map();
      return;
    }
    try {
      const result = await unwrap(commands.getModifiedTimes(props.session.key, paths));
      if (request !== timesRequest) return;
      times.value = new Map(paths.map((p, i) => [p, result[i] ?? null]));
    } catch {
      // Without times the list falls back to alphabetical order.
      if (request === timesRequest) times.value = null;
    }
  },
  { immediate: true },
);

function modifiedText(item: FileItem): string {
  const ms = times.value?.get(item.path) ?? null;
  return ms === null ? "" : formatRelativeTime(ms / 1000);
}

function modifiedTitle(item: FileItem): string | undefined {
  const ms = times.value?.get(item.path) ?? null;
  return ms === null ? undefined : `Modified ${formatAbsoluteTime(ms / 1000)}`;
}

const sections = computed<FileSection[]>(() => {
  if (mode.value === "none") return [];
  const raw = mode.value === "commit" ? commitSections.value : worktreeSections.value;
  return arrangeSections(raw, mode.value, groupBy.value, sortBy.value, times.value);
});

/** With the list grouped by something other than status, staged rows say so themselves. */
function isStagedRow(item: FileItem): boolean {
  return groupBy.value !== "status" && item.id.startsWith("staged:");
}

const flat = computed<FileItem[]>(() => sections.value.flatMap((s) => s.items));

/** The row whose diff is shown: the last one clicked, whatever else is selected. */
function isFocus(item: FileItem): boolean {
  const sel = props.session.selectedFile;
  return sel !== null && sel.path === item.path && sel.target.type === item.target.type;
}

/** More than one row selected, so the focus row gets an outline to show which diff is open. */
const multiple = computed(() => props.session.fileSelection.ids.size > 1);

function showDiff(item: FileItem): void {
  // A conflicted file opens on the comparison last chosen in the diff panel.
  const target: DiffTarget = item.target.type === "conflict" ? { type: "conflict", ...props.session.conflictComparison } : item.target;
  const next: SelectedFile = { path: item.path, oldPath: item.oldPath, target };
  props.session.selectedFile = next;
}

const byId = computed(() => new Map(flat.value.map((i) => [i.id, i])));

const list = useListSelection({
  order: () => flat.value.map((i) => i.id),
  get: () => props.session.fileSelection,
  set: (next) => {
    props.session.fileSelection = next;
  },
  onActivate: (id) => {
    const item = byId.value.get(id);
    if (item) showDiff(item);
  },
  reveal: (id) => {
    const item = byId.value.get(id);
    if (item) document.getElementById(rowId(item))?.scrollIntoView({ block: "nearest" });
  },
  initialCursor: () => flat.value.find((i) => isFocus(i))?.id ?? null,
});

function isSelected(item: FileItem): boolean {
  return list.isSelected(item.id);
}

// Drop rows that are no longer in the list after a status refresh, and the diff with them.
watch(flat, (items) => {
  list.prune();
  const sel = props.session.selectedFile;
  if (!sel || mode.value === "none") return;
  if (!items.some((i) => i.path === sel.path && i.target.type === sel.target.type)) {
    props.session.selectedFile = null;
  }
});

// Selecting a commit or the Working Tree row selects the first file of its list, which shows its
// diff, once the list has loaded. Once per row: a status refresh in the working tree later never
// moves the selection, and a file the user has already clicked is left alone.
let preselectPending = true;
watch(
  () => selectedRow.value?.oid ?? null,
  () => {
    preselectPending = true;
  },
);
watch(
  [flat, changesLoading, () => props.session.status],
  () => {
    if (!preselectPending || mode.value === "none") return;
    if (mode.value === "commit" && changesLoading.value) return;
    if (mode.value === "worktree" && !props.session.status) return;
    preselectPending = false;
    const first = flat.value[0];
    if (first && props.session.fileSelection.ids.size === 0) list.selectOnly(first.id);
  },
  { immediate: true },
);

function onKeydown(event: KeyboardEvent): void {
  list.onKeydown(event);
}

// Ctrl+A selects every file in the active tab's list. App.vue stops the browser from selecting
// the page text; this listener only does the selecting. Text fields keep their own Ctrl+A.
useEventListener(window, "keydown", (event: KeyboardEvent) => {
  if (!isSelectAllKey(event) || isEditableTarget(event.target)) return;
  if (sessions.activeKey !== props.session.key || dialogs.active !== null || dialogs.addRepoOpen) return;
  list.selectAll();
});

function rowId(item: FileItem): string {
  return `file-${props.session.key.slice(0, 8)}-${item.id.replace(/[^a-zA-Z0-9_-]/g, "_")}`;
}

const totalCount = computed(() => flat.value.length);

// ---- Right-click menu ----------------------------------------------------------------------
//
// One menu for the whole list. Right-clicking a row that is not selected selects just that row
// first, so the menu always acts on what is highlighted; right-clicking inside a selection keeps it.

/** The row the menu was opened on, for Open, Show in Folder and the Copy items. */
const menuItem = ref<FileItem | null>(null);

function onRowContextMenu(item: FileItem): void {
  menuItem.value = item;
  if (!list.isSelected(item.id)) list.selectOnly(item.id);
}

function onListContextMenu(event: MouseEvent): void {
  if (!(event.target as HTMLElement | null)?.closest("[data-file-row]")) menuItem.value = null;
}

const menuPaths = computed(() => selectedWorktreePaths(props.session));
const stageReason = computed(() => fileActionReason(props.session, "stage"));
const unstageReason = computed(() => fileActionReason(props.session, "unstage"));
const discardReason = computed(() => fileActionReason(props.session, "discard"));
function countLabel(files: string[]): string {
  return files.length > 1 ? ` ${files.length} Files` : "";
}
/** The selected conflicted rows, and the names of the two sides they can be resolved to. */
const menuConflicts = computed(() => selectedConflictItems(props.session));
const sideNames = computed(() => operationNames(props.session.status, props.session.refs));
/** "Take the Version from main", noting when every selected file was deleted on that side. */
function takeLabel(side: ResolveSide): string {
  const name = side === "ours" ? sideNames.value.ours : sideNames.value.theirs;
  const items = menuConflicts.value;
  const deletes = items.length > 0 && items.every((i) => !conflictStages(i.letter)[side]);
  return `Take the Version from ${name}${deletes ? " (Delete the File)" : ""}${countLabel(items.map((i) => i.path))}`;
}

/** Open and Show in Folder use the file on disk; a deleted file is not there. */
const openReason = computed(() => {
  const item = menuItem.value;
  if (!item) return "Right-click a file.";
  if (item.state === "deleted" && mode.value === "worktree") return "The file has been deleted from the working tree.";
  return openFileReason(props.session);
});

// ---- Name and path columns -----------------------------------------------------------------
//
// Unless the list is grouped by folder (where the heading has the path), each row shows the file
// name in a column of one shared width, then its folder, so names are never pushed out of view
// by long paths and the paths line up.

const tabular = computed(() => groupBy.value !== "folder");
const listEl = ref<HTMLElement | null>(null);
const listWidth = ref(0);
useResizeObserver(listEl, () => {
  listWidth.value = listEl.value?.clientWidth ?? 0;
});

/** The widest file name, at least 6rem and at most 55% of the list. */
const nameColumnPx = computed(() => {
  void fontsVersion.value;
  const widest = Math.ceil(rowTextWidth(flat.value.map((i) => splitPath(i.path).name))) + 4;
  const cap = listWidth.value > 0 ? Math.floor(listWidth.value * 0.55) : widest;
  return Math.max(remToPx(6), Math.min(widest, cap));
});

/** The folder column: the file's folder, or where a renamed file came from. */
function pathText(item: FileItem): string {
  if (item.oldPath && item.oldPath !== item.path) return `from ${item.oldPath}`;
  return splitPath(item.path).dir.replace(/\/$/, "");
}
</script>

<template>
  <section class="area flex h-full min-h-0 flex-col outline-none" tabindex="0" @keydown="onKeydown">
    <PanelHeader title="Files">
      <span v-if="mode === 'worktree'" class="truncate text-xs text-muted-foreground">Working tree</span>
      <span v-else-if="mode === 'commit' && selectedRow" class="truncate font-mono text-xs text-muted-foreground select-text">
        {{ shortOid(selectedRow.oid) }}
      </span>
      <template #actions>
        <div v-if="mode !== 'none'" class="flex items-center gap-1.5 text-[0.7rem] text-muted-foreground">
          <span class="tabular-nums">{{ totalCount }} files</span>
          <label class="inline-flex items-center gap-1">
            Group
            <select v-model="groupBy" class="well h-[17px] rounded-md border px-1 text-[0.7rem] text-foreground" title="Group the files by">
              <option value="status">Status</option>
              <option value="folder">Folder</option>
              <option value="none">No grouping</option>
            </select>
          </label>
          <label class="inline-flex items-center gap-1">
            Sort
            <select v-model="sortBy" class="well h-[17px] rounded-md border px-1 text-[0.7rem] text-foreground" title="Sort the files by">
              <option value="name">Name</option>
              <option value="status">Git status</option>
              <option value="modified" :disabled="mode !== 'worktree'" :title="mode !== 'worktree' ? 'The files in a commit all share its date' : undefined">Date modified</option>
            </select>
          </label>
        </div>
      </template>
    </PanelHeader>

    <EmptyState v-if="mode === 'none'" message="Select a commit or the working tree to list its files." />

    <template v-else>
      <!-- Commit header -->
      <div v-if="mode === 'commit' && selectedRow" class="shrink-0 border-b border-border-soft bg-muted px-2 pt-[5px] pb-1.5 text-base">
        <div class="flex items-center gap-2 text-xs text-muted-foreground">
          <span class="font-mono text-foreground select-text" :title="selectedRow.oid">{{ shortOid(selectedRow.oid) }}</span>
          <span class="truncate" :title="`${selectedRow.authorName} <${selectedRow.authorEmail}>`">{{ selectedRow.authorName }}</span>
          <span class="ml-auto shrink-0 tabular-nums" :title="isoTime(selectedRow.authorTime)">{{ dateText }}</span>
        </div>
        <!-- The More toggle sits in the block's bottom right corner, inside whichever part comes
             last: floated at the end of the subject, beside the message body, or at the end of
             the Compare with row. -->
        <p class="mt-0.5 flow-root break-words select-text">
          {{ selectedRow.subject }}
          <MessageToggle v-if="!messageExpanded && !isMerge" class="float-right ml-2" :expanded="false" @toggle="toggleMessage" />
        </p>
        <div v-if="messageExpanded" class="flex items-end gap-2 pt-1">
          <div class="min-w-0 flex-1">
            <p v-if="messageLoading" class="text-xs text-muted-foreground">Loading message…</p>
            <p v-else-if="messageError" class="text-xs text-destructive">{{ messageError }}</p>
            <pre v-else-if="messageBody.length > 0" class="max-h-40 overflow-auto font-sans text-xs whitespace-pre-wrap text-muted-foreground select-text scrollbar-thin">{{ messageBody }}</pre>
            <p v-else class="text-xs text-muted-foreground italic">No further message body.</p>
          </div>
          <MessageToggle v-if="!isMerge" :expanded="true" @toggle="toggleMessage" />
        </div>
        <div v-if="isMerge" class="mt-1 flex min-w-0 items-center gap-1 text-[0.7rem] text-muted-foreground">
          <template v-if="unchangedParents === null" />
          <template v-else-if="straightMergeParent !== null">
            <GitBranchIcon class="size-3 shrink-0" aria-hidden="true" />
            <span class="min-w-0 truncate" :title="straightMergeText">{{ straightMergeText }}</span>
          </template>
          <template v-else>
            <GitForkIcon class="size-3 shrink-0" aria-hidden="true" />
            <span class="shrink-0">Compare with</span>
            <button
              v-for="(p, i) in commitParents"
              :key="p"
              type="button"
              :aria-pressed="parentIndex === i"
              class="toggle h-[18px] min-w-0 truncate rounded-sm border px-2 leading-none font-medium transition-colors"
              :title="`${parentLabels[i]} (parent ${i + 1})\n${p}`"
              @click="parentIndex = i"
            >
              {{ parentLabels[i] }}
            </button>
          </template>
          <MessageToggle class="ml-auto pl-2" :expanded="messageExpanded" @toggle="toggleMessage" />
        </div>
      </div>

      <ContextMenu>
        <ContextMenuTrigger as-child>
          <div ref="listEl" class="min-h-0 flex-1 overflow-auto scrollbar-thin select-none" @contextmenu="onListContextMenu">
            <div v-if="mode === 'commit' && changesLoading && changes.length === 0" class="flex h-(--row-h) items-center gap-1.5 px-2 text-base text-muted-foreground">
              <LoaderCircleIcon class="size-3 animate-spin" /> Loading changes…
            </div>
            <p v-else-if="changesError" class="px-2 py-1 text-base text-destructive">{{ changesError }}</p>
            <EmptyState v-else-if="flat.length === 0 && mode === 'worktree'" message="The working tree is clean. Nothing to commit." />
            <p v-else-if="flat.length === 0" class="px-2 py-2 text-base text-muted-foreground italic">No files changed.</p>

            <template v-for="section in sections" :key="section.key">
              <template v-if="section.items.length > 0">
                <div
                  v-if="groupBy !== 'none'"
                  :class="
                    cn(
                      'sticky top-0 z-10 flex h-(--row-h) items-center gap-1.5 border-y border-border-soft bg-table-head px-2 select-none',
                      // Folder names keep their case; only the fixed status headings use small capitals.
                      groupBy === 'folder' ? 'font-mono text-xs text-muted-foreground' : 'smallcaps',
                    )
                  "
                >
                  {{ section.title }}
                  <span class="tabular-nums">({{ section.items.length }})</span>
                </div>
                <button
                  v-for="item in section.items"
                  :id="rowId(item)"
                  :key="item.id"
                  type="button"
                  :class="
                    cn(
                      'flex h-(--row-h) w-full items-center gap-1.5 px-2 text-left text-base outline-none',
                      isSelected(item) ? STATE_CLASSES[item.state].band : 'hover:bg-row-hover',
                      multiple && isFocus(item) && 'shadow-[inset_2px_0_0_var(--ring)]',
                    )
                  "
                  :aria-selected="isSelected(item)"
                  :title="item.oldPath ? `${item.oldPath} → ${item.path}` : item.path"
                  data-file-row
                  @mousedown="list.onMousedown"
                  @click="list.onClick(item.id, $event)"
                  @contextmenu="onRowContextMenu(item)"
                >
                  <span
                    :class="cn('inline-flex h-4 min-w-4 shrink-0 items-center justify-center rounded-sm px-0.5 font-mono text-[0.7rem] leading-none font-semibold', STATE_CLASSES[item.state].badge)"
                  >
                    {{ item.letter }}
                  </span>
                  <template v-if="tabular">
                    <span class="shrink-0 truncate" :style="{ width: `${nameColumnPx}px` }">{{ splitPath(item.path).name }}</span>
                    <!-- Truncates from the start, so the end of the path, nearest the file, stays visible. -->
                    <span :class="cn('files-path min-w-0 flex-1 truncate text-[0.85em]', isSelected(item) ? 'text-band-dir' : 'text-muted-foreground')">
                      <span dir="ltr">{{ pathText(item) }}</span>
                    </span>
                  </template>
                  <span v-else class="min-w-0 flex-1 truncate">
                    <template v-if="item.oldPath && item.oldPath !== item.path">
                      <span :class="isSelected(item) ? 'text-band-dir' : 'text-muted-foreground'">{{ item.oldPath }}</span>
                      <span :class="cn('mx-1', isSelected(item) ? 'text-band-dir' : 'text-muted-foreground')">→</span>
                    </template>
                    <!-- Grouped by folder, the folder is in the section heading, so the row shows only the name. -->
                    <span v-if="item.oldPath" :class="isSelected(item) ? 'text-band-dir' : 'text-muted-foreground'">{{ splitPath(item.path).dir }}</span><span>{{ splitPath(item.path).name }}</span>
                  </span>
                  <span v-if="isStagedRow(item)" class="shrink-0 text-[0.7rem] text-muted-foreground">staged</span>
                  <span v-if="sortingByDate && modifiedText(item)" class="shrink-0 text-[0.7rem] text-muted-foreground tabular-nums" :title="modifiedTitle(item)">
                    {{ modifiedText(item) }}
                  </span>
                  <span v-if="item.binary" class="shrink-0 text-[0.7rem] text-muted-foreground">binary</span>
                  <span v-else-if="item.added !== null || item.deleted !== null" class="shrink-0 font-mono text-[0.7rem] tabular-nums">
                    <span class="text-diff-add-fg">+{{ item.added ?? 0 }}</span>
                    <span class="ml-1 text-diff-del-fg">−{{ item.deleted ?? 0 }}</span>
                  </span>
                </button>
              </template>
            </template>
          </div>
        </ContextMenuTrigger>
        <ContextMenuContent class="min-w-56">
          <template v-if="mode === 'worktree' && menuConflicts.length > 0">
            <ActionMenuItem :reason="null" data-resolve="ours" @select="takeVersion(session, menuConflicts, 'ours')">{{ takeLabel("ours") }}</ActionMenuItem>
            <ActionMenuItem :reason="null" data-resolve="theirs" @select="takeVersion(session, menuConflicts, 'theirs')">{{ takeLabel("theirs") }}</ActionMenuItem>
            <ActionMenuItem :reason="null" data-resolve="mark" @select="markResolved(session, menuConflicts)">
              Mark as Resolved{{ countLabel(menuConflicts.map((i) => i.path)) }}
            </ActionMenuItem>
            <ContextMenuSeparator />
          </template>
          <template v-if="mode === 'worktree'">
            <ActionMenuItem :reason="stageReason" @select="stageSelected(session)">Stage{{ countLabel(menuPaths.stageFiles) }}</ActionMenuItem>
            <ActionMenuItem :reason="unstageReason" @select="unstageSelected(session)">Unstage{{ countLabel(menuPaths.unstageFiles) }}</ActionMenuItem>
            <ActionMenuItem :reason="discardReason" destructive @select="discardSelected(session)">
              Discard Changes{{ countLabel(menuPaths.discardFiles) }}…
            </ActionMenuItem>
            <ContextMenuSeparator />
          </template>
          <ActionMenuItem :reason="openReason" @select="menuItem && openWorktreeFile(session.key, menuItem.path)">
            {{ mode === "commit" ? "Open the Working Tree Version" : "Open" }}
          </ActionMenuItem>
          <ActionMenuItem :reason="openReason" @select="menuItem && revealWorktreeFile(session.key, menuItem.path)">Show in Folder</ActionMenuItem>
          <ContextMenuSeparator />
          <ActionMenuItem :reason="menuItem ? null : 'Right-click a file.'" @select="menuItem && copyText(menuItem.path, 'the path')">Copy Path</ActionMenuItem>
          <ActionMenuItem :reason="menuItem ? null : 'Right-click a file.'" @select="menuItem && copyFullPath(session, menuItem.path)">Copy Full Path</ActionMenuItem>
        </ContextMenuContent>
      </ContextMenu>
    </template>
  </section>
</template>

<style>
/* Right-to-left layout puts the ellipsis at the start of the path; the inner dir="ltr" span keeps
   the path itself in its normal order. */
.files-path {
  direction: rtl;
  text-align: left;
}
</style>
