<script setup lang="ts">
import { computed, nextTick, ref, shallowRef, watch } from "vue";
import { ArrowLeftIcon, HistoryIcon, LoaderCircleIcon } from "@lucide/vue";
import { commands, type BlameCommit, type BlameOrigin, type Eol, type FileBlame } from "@/lib/bindings";
import { Button } from "@/components/ui/button";
import { ContextMenu, ContextMenuContent, ContextMenuSeparator, ContextMenuTrigger } from "@/components/ui/context-menu";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import ActionMenuItem from "@/components/graph/ActionMenuItem.vue";
import EmptyState from "@/components/shell/EmptyState.vue";
import PanelHeader from "@/components/shell/PanelHeader.vue";
import { useVirtualRows } from "@/composables/useVirtualRows";
import { layoutBlame, revisionLabel } from "@/lib/blame";
import { diffRowPx, diffTextStyle, displayLength, resolveDiffFont } from "@/lib/diff/font";
import { openFileHistory, showCommitInGraph } from "@/lib/fileViews";
import { splitPath } from "@/lib/files";
import { formatAbsoluteTime, formatRelativeTime, hashString, isoTime, shortOid } from "@/lib/format";
import { laneColour, PALETTE_SIZE } from "@/lib/graph/paths";
import { errorMessage, unwrap } from "@/lib/ipc";
import { copyText } from "@/lib/repoActions";
import { cn } from "@/lib/utils";
import { useDialogsStore } from "@/stores/dialogs";
import { useSettingsStore } from "@/stores/settings";

const dialogs = useDialogsStore();
const settings = useSettingsStore();

const request = computed(() => dialogs.current("blame"));

/** One version of a file the dialog shows: `rev` null is the working tree. */
interface BlameView {
  path: string;
  rev: string | null;
  /** The line (counting from 1) to scroll into view once loaded. */
  line?: number;
}

/** The version shown, and the ones "Blame Before This Change" left, for Back. */
const view = ref<BlameView | null>(null);
const backStack = ref<BlameView[]>([]);

const blame = shallowRef<FileBlame | null>(null);
const loading = ref(false);
const error = ref<string | null>(null);
/** The line whose commit was clicked; its block and its commit are highlighted. */
const pickedLine = ref<number | null>(null);
let loadRequest = 0;

watch(request, (r) => {
  backStack.value = [];
  view.value = r ? { path: r.path, rev: r.rev, line: r.line } : null;
});

watch(view, () => void load());

async function load(): Promise<void> {
  const r = request.value;
  const v = view.value;
  const id = (loadRequest += 1);
  pickedLine.value = null;
  // Cleared rather than kept while the next version loads: what the dialog says about it (the
  // header, a missing file's message) already describes the next version.
  blame.value = null;
  if (!r || !v) return;
  loading.value = true;
  error.value = null;
  try {
    const result = await unwrap(commands.getFileBlame(r.key, v.path, v.rev));
    if (id !== loadRequest) return;
    blame.value = result;
    await nextTick();
    const line = v.line ?? 1;
    // A few lines of context above the line asked for.
    virtual.scrollTo(Math.max(0, line - 4) * rowPx.value);
  } catch (e) {
    if (id !== loadRequest) return;
    blame.value = null;
    error.value = errorMessage(e);
  } finally {
    if (id === loadRequest) loading.value = false;
  }
}

function onOpenChange(open: boolean): void {
  if (!open) dialogs.cancel();
}

/** Shows another version of the file, remembering this one (and where it was scrolled to) for Back. */
function go(next: BlameView): void {
  const v = view.value;
  if (v) backStack.value = [...backStack.value, { ...v, line: Math.floor(virtual.scrollTop.value / rowPx.value) + 4 }];
  view.value = next;
}

function goBack(): void {
  const stack = backStack.value;
  const previous = stack[stack.length - 1];
  if (!previous) return;
  backStack.value = stack.slice(0, -1);
  view.value = previous;
}

// ---- Layout and drawing --------------------------------------------------------------------

/** Font, size, line height and tab width from the `diff` section of settings.json, as the diff viewer uses. */
const font = computed(() => resolveDiffFont(settings.settings?.diff));
const textStyle = computed(() => ({ ...diffTextStyle(font.value), "--blame-commit-w": "30rem" }));
const rowPx = computed(() => diffRowPx(font.value));

const layout = computed(() => (blame.value?.content === "lines" ? layoutBlame(blame.value) : null));
const lineCount = computed(() => (layout.value ? (blame.value?.lines.length ?? 0) : 0));
/** The longest line in characters with tabs expanded, so the rows are wide enough to scroll to its end. */
const maxLength = computed(() => {
  const tab = font.value.tabWidth;
  let max = 0;
  for (const line of layout.value ? (blame.value?.lines ?? []) : []) max = Math.max(max, displayLength(line, tab));
  return max;
});
const contentMinWidth = computed(() => `calc(var(--blame-commit-w) + var(--diff-num-w) + ${Math.max(20, maxLength.value + 3)}ch)`);

const scroller = ref<HTMLElement | null>(null);
const virtual = useVirtualRows(scroller, { rowHeight: () => rowPx.value, total: () => lineCount.value, overscan: 10 });
/** A line drawn in the list, with its commit and whether it starts that commit's block. */
interface Row {
  index: number;
  text: string;
  commit: BlameCommit | null;
  blockStart: boolean;
}

const visibleRows = computed<Row[]>(() => {
  const lines = blame.value?.lines ?? [];
  const rows: Row[] = [];
  for (let index = virtual.start.value; index < Math.min(virtual.end.value, lineCount.value); index += 1) {
    rows.push({ index, text: lines[index] ?? "", commit: commitOf(index), blockStart: isBlockStart(index) });
  }
  return rows;
});

function commitOf(line: number): BlameCommit | null {
  const l = layout.value;
  const block = l?.blocks[l.blockOf[line] ?? 0];
  return block ? (blame.value?.commits[block.commit] ?? null) : null;
}

function originOf(line: number): BlameOrigin | null {
  const l = layout.value;
  const group = l ? blame.value?.groups[l.groupOf[line] ?? 0] : undefined;
  return group ? (blame.value?.origins[group.origin] ?? null) : null;
}

function isBlockStart(line: number): boolean {
  const l = layout.value;
  return !!l && l.blocks[l.blockOf[line] ?? 0]?.start === line;
}

/**
 * The commit column's background: the text colour mixed in, more of it the newer the commit, so
 * recent changes stand out in either theme; lines not committed yet in the "modified" colour.
 * Grey rather than a hue, so that the selected commit's colour is never mistaken for an age.
 */
function shade(line: number): string {
  const l = layout.value;
  const block = l?.blocks[l.blockOf[line] ?? 0];
  if (!l || !block) return "transparent";
  if (blame.value?.commits[block.commit]?.uncommitted) return "color-mix(in srgb, var(--color-modified) 24%, var(--background))";
  const age = l.age[block.commit] ?? 0;
  return `color-mix(in srgb, var(--foreground) ${Math.round(2 + age * 16)}%, var(--background))`;
}

/** Each commit's bar takes a lane colour from its ID, so neighbouring blocks from different commits tell apart. */
function barColour(commit: BlameCommit): string {
  return commit.uncommitted ? "var(--color-modified)" : laneColour(hashString(commit.oid) % PALETTE_SIZE);
}

const pickedCommit = computed(() => (pickedLine.value === null ? null : commitOf(pickedLine.value)));
const pickedOrigin = computed(() => (pickedLine.value === null ? null : originOf(pickedLine.value)));

function isPicked(line: number): boolean {
  const c = pickedCommit.value;
  return c !== null && commitOf(line)?.oid === c.oid;
}

const absoluteDates = computed(() => settings.settings?.appearance?.dateFormat === "absolute");
/** The day only, when dates are absolute: the column is narrow, and the tooltip has the full time. */
function dateText(commit: BlameCommit): string {
  return absoluteDates.value ? (formatAbsoluteTime(commit.authorTime).split(" ")[0] ?? "") : formatRelativeTime(commit.authorTime);
}

function commitTitle(commit: BlameCommit): string {
  if (commit.uncommitted) return "Not committed yet: changed in the working tree or the index.";
  const lines = [commit.oid, `${commit.authorName} <${commit.authorEmail}>, ${isoTime(commit.authorTime)}`, commit.summary];
  if (commit.boundary) lines.push("The first commit of the history: these lines may be older than it.");
  lines.push("Click to select this commit's lines; right-click for more.");
  return lines.join("\n");
}

// ---- Actions on the picked commit ---------------------------------------------------------

function pick(line: number): void {
  pickedLine.value = line;
}

const graphReason = computed(() => {
  const c = pickedCommit.value;
  if (!c) return "Click a line's commit first.";
  if (c.uncommitted) return "These lines are not committed yet, so they have no commit in the graph.";
  return null;
});

const beforeReason = computed(() => {
  if (!pickedCommit.value) return "Click a line's commit first.";
  if (!pickedOrigin.value?.previous) return "This commit added the file, so there is no earlier version to blame.";
  return null;
});

const atCommitReason = computed(() => (pickedCommit.value?.uncommitted ? "These lines are not committed yet." : graphReason.value));

function showInGraph(): void {
  const r = request.value;
  const c = pickedCommit.value;
  if (r && c && !graphReason.value) void showCommitInGraph(r.key, c.oid);
}

/** The line in the picked commit's version of the file that the picked line came from. */
function pickedOrigLine(): number | undefined {
  const line = pickedLine.value;
  const l = layout.value;
  if (line === null || !l) return undefined;
  const group = blame.value?.groups[l.groupOf[line] ?? 0];
  return group ? group.origStart + (line - group.start) : undefined;
}

/**
 * Blames the file as it was in the commit before the picked one changed these lines, scrolled to
 * about where the line was: Git reports where it is in the picked commit's version, which is
 * close to where it was before.
 */
function blameBefore(): void {
  const previous = pickedOrigin.value?.previous;
  if (!previous || beforeReason.value) return;
  go({ path: previous.path, rev: previous.oid, line: pickedOrigLine() });
}

function blameAtCommit(): void {
  const c = pickedCommit.value;
  const origin = pickedOrigin.value;
  if (!c || !origin || atCommitReason.value) return;
  go({ path: origin.path, rev: c.oid, line: pickedOrigLine() });
}

function showHistory(): void {
  const r = request.value;
  const v = view.value;
  if (r && v) openFileHistory(r.key, v.path, v.rev);
}

/** The footer's buttons, which act on the picked commit, with why each cannot right now. */
const footerActions = computed(() => [
  { label: "Show in Commit Graph", reason: graphReason.value, run: showInGraph },
  { label: "Blame Before This Change", reason: beforeReason.value, run: blameBefore },
]);

// ---- Text ----------------------------------------------------------------------------------

const title = computed(() => (view.value ? splitPath(view.value.path).name : ""));
const versionText = computed(() => {
  const v = view.value;
  if (!v) return "";
  return v.rev === null ? "as it is in the working tree" : `as it was in commit ${revisionLabel(v.rev)}`;
});

/** What to say in place of the lines, and the button that may help. */
const placeholder = computed<{ message: string; action?: { label: string; view: BlameView } } | null>(() => {
  const b = blame.value;
  const v = view.value;
  if (!b || !v) return null;
  switch (b.content) {
    case "binary":
      return { message: "This is a binary file, so it has no lines to blame." };
    case "notCommitted":
      return { message: "This file is not in any commit yet, so no line has a commit to show." };
    case "missing":
      if (v.rev === null) {
        return { message: "The file is not in the working tree: it has been deleted.", action: { label: "Blame the Version in HEAD", view: { path: v.path, rev: "HEAD" } } };
      }
      return { message: `The file is not in commit ${revisionLabel(v.rev)}.`, action: { label: "Blame the Commit Before", view: { path: v.path, rev: `${v.rev}^` } } };
    case "lines":
      return b.lines.length === 0 ? { message: "The file is empty." } : null;
  }
  return null;
});

const backTitle = computed(() => {
  const previous = backStack.value[backStack.value.length - 1];
  if (!previous) return "";
  return previous.rev === null ? "Back to the working tree version" : `Back to the version in commit ${revisionLabel(previous.rev)}`;
});

const EOL_NAMES: Record<Eol, string | null> = { lf: "LF", crLf: "CRLF", mixed: "Mixed", none: null };
const eolText = computed(() => (blame.value ? EOL_NAMES[blame.value.eol] : null));
const commitCount = computed(() => blame.value?.commits.filter((c) => !c.uncommitted).length ?? 0);
const summaryText = computed(() => {
  if (!layout.value) return "";
  const lines = lineCount.value === 1 ? "1 line" : `${lineCount.value.toLocaleString()} lines`;
  const commits = commitCount.value === 1 ? "1 commit" : `${commitCount.value.toLocaleString()} commits`;
  return `${lines} from ${commits}`;
});
const pickedText = computed(() => {
  const c = pickedCommit.value;
  if (!c) return "Click a line's commit to select it.";
  if (c.uncommitted) return "Not committed yet";
  return `${shortOid(c.oid)} ${c.authorName}, ${dateText(c)}: ${c.summary}`;
});
</script>

<template>
  <Dialog :open="request !== null" @update:open="onOpenChange">
    <DialogContent v-if="request && view" class="h-[80vh] sm:max-w-[80vw]" size-key="blame">
      <DialogHeader>
        <DialogTitle>Blame of {{ title }}</DialogTitle>
        <DialogDescription class="break-all">
          The commit that last changed each line of {{ view.path }}, {{ versionText }}. The newer a block's commit, the stronger its shading.
        </DialogDescription>
      </DialogHeader>

      <div class="-mx-6 -my-5 flex min-h-0 flex-1 flex-col bg-window p-1.5" data-dialog-grow>
        <section class="area flex min-h-0 flex-1 flex-col">
          <PanelHeader title="Lines">
            <span class="truncate text-xs text-muted-foreground select-text" :title="view.path">{{ view.path }}</span>
            <span v-if="eolText && layout" class="shrink-0 rounded-sm border border-border-soft px-1 font-mono text-[0.65rem] leading-[14px] text-muted-foreground" title="Line endings of the file">
              {{ eolText }}
            </span>
            <LoaderCircleIcon v-if="loading" class="size-3 shrink-0 animate-spin text-muted-foreground" />
            <template #actions>
              <span class="mr-1 text-[0.7rem] text-muted-foreground tabular-nums">{{ summaryText }}</span>
              <!-- Newest to oldest, as the commit column shades them. -->
              <span v-if="layout" class="mr-2 inline-flex items-center gap-1 text-[0.7rem] text-muted-foreground" title="Each block of lines is shaded by how recent its commit is among this file's commits">
                Newer
                <span class="h-2 w-12 rounded-sm border border-border-soft bg-linear-to-r from-[color-mix(in_srgb,var(--foreground)_18%,var(--background))] to-[color-mix(in_srgb,var(--foreground)_2%,var(--background))]" />
                Older
              </span>
              <button
                v-if="backStack.length > 0"
                type="button"
                class="toggle inline-flex h-[18px] items-center gap-1 rounded-sm border px-2 text-[0.7rem] leading-none font-medium [&_svg]:size-3"
                :title="backTitle"
                @click="goBack"
              >
                <ArrowLeftIcon />
                Back
              </button>
              <button
                type="button"
                class="toggle inline-flex h-[18px] items-center gap-1 rounded-sm border px-2 text-[0.7rem] leading-none font-medium [&_svg]:size-3"
                title="Every commit that changed this file"
                @click="showHistory"
              >
                <HistoryIcon />
                Show History
              </button>
            </template>
          </PanelHeader>

          <EmptyState v-if="error" :message="error" class="text-destructive" />
          <EmptyState v-else-if="!blame && loading" message="Reading who changed each line…" />
          <EmptyState v-else-if="placeholder" :message="placeholder.message">
            <Button v-if="placeholder.action" type="button" variant="outline" size="sm" @click="go(placeholder.action.view)">{{ placeholder.action.label }}</Button>
          </EmptyState>

          <ContextMenu v-else-if="layout">
            <ContextMenuTrigger as-child>
              <div ref="scroller" class="relative min-h-0 flex-1 overflow-auto select-text scrollbar-thin" :style="textStyle">
                <div class="relative" :style="{ height: `${lineCount * rowPx}px`, minWidth: contentMinWidth }">
                  <!-- No will-change-transform here, for the reason given in DiffViewer.vue. -->
                  <div class="absolute inset-x-0 top-0" :style="{ transform: `translateY(${virtual.start.value * rowPx}px)` }">
                    <div
                      v-for="row in visibleRows"
                      :key="row.index"
                      :class="['blame-row', row.blockStart && row.index > 0 && 'blame-block-start', isPicked(row.index) && 'blame-line-selected']"
                    >
                      <div class="blame-fixed">
                        <div
                          :class="['blame-commit', isPicked(row.index) && 'blame-commit-selected']"
                          :style="{ backgroundColor: shade(row.index) }"
                          :title="row.commit ? commitTitle(row.commit) : undefined"
                          @mousedown.left="pick(row.index)"
                          @contextmenu="pick(row.index)"
                        >
                          <template v-if="row.commit">
                            <span class="blame-commit-bar" :style="{ backgroundColor: barColour(row.commit) }" />
                            <template v-if="row.blockStart && row.commit.uncommitted">
                              <span class="font-mono text-modified-fg">0000000</span>
                              <span class="col-span-3 truncate italic">Not committed yet</span>
                            </template>
                            <template v-else-if="row.blockStart">
                              <span :class="cn('font-mono', row.commit.boundary && 'text-muted-foreground')">{{ shortOid(row.commit.oid) }}</span>
                              <span class="truncate">{{ row.commit.authorName }}</span>
                              <span class="truncate tabular-nums">{{ dateText(row.commit) }}</span>
                              <span class="truncate">{{ row.commit.summary }}</span>
                            </template>
                          </template>
                        </div>
                        <span class="diff-num"><span class="diff-glyphs">{{ row.index + 1 }}</span></span>
                      </div>
                      <span class="diff-code pl-2"><span class="diff-glyphs">{{ row.text }}</span></span>
                    </div>
                  </div>
                </div>
              </div>
            </ContextMenuTrigger>
            <ContextMenuContent class="min-w-56">
              <ActionMenuItem :reason="graphReason" @select="showInGraph">Show in Commit Graph</ActionMenuItem>
              <ActionMenuItem :reason="beforeReason" @select="blameBefore">Blame Before This Change</ActionMenuItem>
              <ActionMenuItem :reason="atCommitReason" @select="blameAtCommit">Blame at This Commit</ActionMenuItem>
              <ContextMenuSeparator />
              <ActionMenuItem :reason="graphReason" @select="pickedCommit && copyText(pickedCommit.oid, 'the commit ID')">Copy Commit ID</ActionMenuItem>
            </ContextMenuContent>
          </ContextMenu>
        </section>
      </div>

      <DialogFooter>
        <span class="mr-auto min-w-0 truncate text-xs text-muted-foreground" :title="pickedCommit && !pickedCommit.uncommitted ? pickedCommit.oid : undefined">{{ pickedText }}</span>
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
