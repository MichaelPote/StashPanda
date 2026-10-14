<script setup lang="ts">
import { computed, ref, shallowRef, watch } from "vue";
import { LoaderCircleIcon } from "@lucide/vue";
import { commands, type FileHistoryEntry } from "@/lib/bindings";
import { Button } from "@/components/ui/button";
import { ContextMenu, ContextMenuContent, ContextMenuSeparator, ContextMenuTrigger } from "@/components/ui/context-menu";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { ResizableHandle, ResizablePanel, ResizablePanelGroup } from "@/components/ui/resizable";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import DiffViewer from "@/components/diff/DiffViewer.vue";
import ActionMenuItem from "@/components/graph/ActionMenuItem.vue";
import EmptyState from "@/components/shell/EmptyState.vue";
import PanelHeader from "@/components/shell/PanelHeader.vue";
import { useVirtualRows } from "@/composables/useVirtualRows";
import { revisionLabel } from "@/lib/blame";
import { openBlame, showCommitInGraph } from "@/lib/fileViews";
import { STATE_CLASSES, letterState, splitPath, statusLetter } from "@/lib/files";
import { formatAbsoluteTime, formatRelativeTime, isoTime, shortOid } from "@/lib/format";
import { ROW_HEIGHT } from "@/lib/graph/paths";
import { errorMessage, unwrap } from "@/lib/ipc";
import { copyText } from "@/lib/repoActions";
import { cn } from "@/lib/utils";
import { useDialogsStore } from "@/stores/dialogs";
import { useSessionsStore, type SelectedFile } from "@/stores/sessions";
import { useSettingsStore } from "@/stores/settings";

const dialogs = useDialogsStore();
const sessions = useSessionsStore();
const settings = useSettingsStore();

/** Commits read at a time. `git log --follow` cannot skip, so each "Load More" reads from the top again, this many further. */
const PAGE = 500;

const request = computed(() => dialogs.current("fileHistory"));
const session = computed(() => (request.value ? (sessions.sessions.get(request.value.key) ?? null) : null));

const entries = shallowRef<FileHistoryEntry[]>([]);
const more = ref(false);
const limit = ref(PAGE);
const loading = ref(false);
const error = ref<string | null>(null);
/** The selected commit, by oid, so it stays selected when more commits load. */
const selectedOid = ref<string | null>(null);
let loadRequest = 0;

async function load(): Promise<void> {
  const r = request.value;
  if (!r) return;
  const id = (loadRequest += 1);
  loading.value = true;
  error.value = null;
  try {
    const history = await unwrap(commands.getFileHistory(r.key, r.path, r.rev, limit.value));
    if (id !== loadRequest) return;
    entries.value = history.entries;
    more.value = history.more;
    if (selectedOid.value === null || !history.entries.some((e) => e.oid === selectedOid.value)) {
      selectedOid.value = history.entries[0]?.oid ?? null;
    }
  } catch (e) {
    if (id !== loadRequest) return;
    error.value = errorMessage(e);
  } finally {
    if (id === loadRequest) loading.value = false;
  }
}

watch(request, (r) => {
  entries.value = [];
  more.value = false;
  limit.value = PAGE;
  selectedOid.value = null;
  error.value = null;
  if (r) void load();
});

function loadMore(): void {
  limit.value += PAGE;
  void load();
}

function onOpenChange(open: boolean): void {
  if (!open) dialogs.cancel();
}

const selectedIndex = computed(() => entries.value.findIndex((e) => e.oid === selectedOid.value));
const selected = computed<FileHistoryEntry | null>(() => entries.value[selectedIndex.value] ?? null);

/** The selected commit's diff of the file. A merge is compared with its first parent, the branch it was merged into. */
const selectedFile = computed<SelectedFile | null>(() => {
  const e = selected.value;
  if (!e) return null;
  return {
    path: e.path,
    oldPath: e.oldPath,
    target: { type: "commit", commit: e.oid, parent: e.parents.length > 1 ? (e.parents[0] ?? null) : null },
  };
});

// ---- The list --------------------------------------------------------------------------------

const scroller = ref<HTMLElement | null>(null);
const virtual = useVirtualRows(scroller, { rowHeight: ROW_HEIGHT, total: () => entries.value.length, overscan: 10 });
const visible = computed(() => entries.value.slice(virtual.start.value, virtual.end.value));

function select(index: number): void {
  const e = entries.value[index];
  if (!e) return;
  selectedOid.value = e.oid;
  virtual.ensureVisible(index);
}

function onKeydown(event: KeyboardEvent): void {
  const count = entries.value.length;
  if (count === 0) return;
  const current = Math.max(0, selectedIndex.value);
  const page = virtual.rowsPerPage();
  const next: Record<string, number> = {
    ArrowUp: current - 1,
    ArrowDown: current + 1,
    PageUp: current - page,
    PageDown: current + page,
    Home: 0,
    End: count - 1,
  };
  const target = next[event.key];
  if (target === undefined) return;
  event.preventDefault();
  select(Math.min(count - 1, Math.max(0, target)));
}

const absoluteDates = computed(() => settings.settings?.appearance?.dateFormat === "absolute");
function dateText(e: FileHistoryEntry): string {
  return absoluteDates.value ? formatAbsoluteTime(e.authorTime) : formatRelativeTime(e.authorTime);
}

/** The status letter's badge: a rename shows R, a merge M. */
function badge(e: FileHistoryEntry): { letter: string; className: string } {
  const letter = statusLetter(e.status);
  return { letter, className: STATE_CLASSES[letterState(letter)].badge };
}

/** Where the file was in this commit, when that differs from the path the dialog was opened for. */
function pathNote(e: FileHistoryEntry): string | null {
  if (e.oldPath && e.oldPath !== e.path) return `${e.oldPath} → ${e.path}`;
  return e.path !== request.value?.path ? e.path : null;
}

function rowTitle(e: FileHistoryEntry): string {
  const lines = [`${e.oid}`, `${e.authorName} <${e.authorEmail}>, ${isoTime(e.authorTime)}`, e.subject];
  if (e.parents.length > 1) lines.push("A merge: its diff is against its first parent.");
  return lines.join("\n");
}

// ---- Actions on the selected commit ----------------------------------------------------------

const graphReason = computed(() => (selected.value ? null : "Select a commit first."));
const blameReason = computed(() => {
  const e = selected.value;
  if (!e) return "Select a commit first.";
  if (e.status === "deleted") return "This commit deleted the file, so it has no lines to blame.";
  return null;
});

function showInGraph(): void {
  const r = request.value;
  const e = selected.value;
  if (r && e) void showCommitInGraph(r.key, e.oid);
}

function blameAtCommit(): void {
  const r = request.value;
  const e = selected.value;
  if (r && e && !blameReason.value) openBlame(r.key, e.path, e.oid);
}

/** The footer's buttons, which act on the selected commit, with why each cannot right now. */
const footerActions = computed(() => [
  { label: "Show in Commit Graph", reason: graphReason.value, run: showInGraph },
  { label: "Blame at This Commit", reason: blameReason.value, run: blameAtCommit },
]);

/** Right-clicking a row selects it, so the menu acts on what is highlighted. */
function onRowContextMenu(index: number): void {
  select(index);
}

const title = computed(() => (request.value ? splitPath(request.value.path).name : ""));
const fromText = computed(() => {
  const r = request.value;
  if (!r) return "";
  return r.rev === null ? "HEAD" : `commit ${revisionLabel(r.rev)}`;
});
const countText = computed(() => {
  const n = entries.value.length;
  const commits = n === 1 ? "1 commit" : `${n.toLocaleString()} commits`;
  return more.value ? `The newest ${commits}` : commits;
});
</script>

<template>
  <Dialog :open="request !== null" @update:open="onOpenChange">
    <DialogContent v-if="request" class="h-[80vh] sm:max-w-[80vw]" size-key="fileHistory">
      <DialogHeader>
        <DialogTitle>History of {{ title }}</DialogTitle>
        <DialogDescription class="break-all">
          Every commit that changed {{ request.path }}, newest first, from {{ fromText }}. Renames are followed back to the file's earlier names.
        </DialogDescription>
      </DialogHeader>

      <!-- The commits above and the selected commit's diff below, on the window colour like the main window's panels. -->
      <div class="-mx-6 -my-5 flex min-h-0 flex-1 flex-col bg-window p-1.5" data-dialog-grow>
        <ResizablePanelGroup direction="vertical" class="min-h-0 flex-1">
          <ResizablePanel :default-size="38" :min-size="15">
            <section class="area flex h-full min-h-0 flex-col">
              <PanelHeader title="Commits">
                <LoaderCircleIcon v-if="loading" class="size-3 shrink-0 animate-spin text-muted-foreground" />
                <template #actions>
                  <span class="text-[0.7rem] text-muted-foreground tabular-nums">{{ countText }}</span>
                </template>
              </PanelHeader>
              <EmptyState v-if="error" :message="error" class="text-destructive" />
              <EmptyState v-else-if="!loading && entries.length === 0" message="No commit has changed this file yet." />
              <ContextMenu v-else>
                <ContextMenuTrigger as-child>
                  <div
                    ref="scroller"
                    class="relative min-h-0 flex-1 overflow-auto outline-none scrollbar-thin select-none"
                    tabindex="0"
                    aria-label="Commits that changed the file"
                    @keydown="onKeydown"
                  >
                    <div class="relative" :style="{ height: `${entries.length * ROW_HEIGHT}px` }">
                      <div class="absolute inset-x-0 top-0" :style="{ transform: `translateY(${virtual.start.value * ROW_HEIGHT}px)` }">
                        <div
                          v-for="(e, k) in visible"
                          :key="e.oid"
                          :class="
                            cn(
                              'grid h-(--row-h) cursor-default grid-cols-[4.5rem_1.25rem_minmax(0,1fr)_minmax(6rem,11rem)_7.5rem] items-center gap-2 px-2 text-base',
                              e.oid === selectedOid ? 'row-selected' : 'hover:bg-row-hover',
                            )
                          "
                          :title="rowTitle(e)"
                          :aria-selected="e.oid === selectedOid"
                          @mousedown="select(virtual.start.value + k)"
                          @contextmenu="onRowContextMenu(virtual.start.value + k)"
                        >
                          <span class="font-mono text-xs">{{ shortOid(e.oid) }}</span>
                          <span :class="cn('inline-flex h-4 min-w-4 items-center justify-center rounded-sm px-0.5 font-mono text-[0.7rem] leading-none font-semibold', badge(e).className)">
                            {{ badge(e).letter }}
                          </span>
                          <span class="flex min-w-0 items-baseline gap-2">
                            <span class="truncate">{{ e.subject }}</span>
                            <span v-if="pathNote(e)" class="min-w-0 shrink truncate text-[0.7rem] text-muted-foreground">{{ pathNote(e) }}</span>
                          </span>
                          <span class="truncate text-xs text-muted-foreground">{{ e.authorName }}</span>
                          <span class="truncate text-right text-xs text-muted-foreground tabular-nums">{{ dateText(e) }}</span>
                        </div>
                      </div>
                    </div>
                  </div>
                </ContextMenuTrigger>
                <ContextMenuContent class="min-w-56">
                  <ActionMenuItem :reason="graphReason" @select="showInGraph">Show in Commit Graph</ActionMenuItem>
                  <ActionMenuItem :reason="blameReason" @select="blameAtCommit">Blame at This Commit</ActionMenuItem>
                  <ContextMenuSeparator />
                  <ActionMenuItem :reason="graphReason" @select="selected && copyText(selected.oid, 'the commit ID')">Copy Commit ID</ActionMenuItem>
                </ContextMenuContent>
              </ContextMenu>
            </section>
          </ResizablePanel>
          <ResizableHandle />
          <ResizablePanel :default-size="62" :min-size="20">
            <DiffViewer v-if="session" :session="session" :file="selectedFile" :file-actions="false" />
          </ResizablePanel>
        </ResizablePanelGroup>
      </div>

      <DialogFooter>
        <div class="mr-auto flex items-center gap-2 text-xs text-muted-foreground">
          <span class="tabular-nums">{{ countText }}</span>
          <button v-if="more" type="button" class="font-medium text-primary hover:underline disabled:opacity-50" :disabled="loading" @click="loadMore">
            Load {{ PAGE }} More
          </button>
        </div>
        <Tooltip v-for="action in footerActions" :key="action.label" :disabled="action.reason === null">
          <TooltipTrigger as-child>
            <!-- A disabled button swallows pointer events, so the wrapper span carries the tooltip. -->
            <span class="inline-flex">
              <Button type="button" variant="outline" :disabled="action.reason !== null" @click="action.run">{{ action.label }}</Button>
            </span>
          </TooltipTrigger>
          <TooltipContent side="top">{{ action.reason }}</TooltipContent>
        </Tooltip>
        <Button type="button" @click="dialogs.cancel()">Close</Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
</template>
