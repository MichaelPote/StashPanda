<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { ChevronDownIcon, ChevronRightIcon, LoaderCircleIcon } from "@lucide/vue";
import { commands, type ChangeStatus, type DiffTarget, type FileChange, type StatusEntry } from "@/lib/bindings";
import PanelHeader from "@/components/shell/PanelHeader.vue";
import EmptyState from "@/components/shell/EmptyState.vue";
import { formatAbsoluteTime, formatRelativeTime, isoTime, shortOid } from "@/lib/format";
import { errorMessage, unwrap } from "@/lib/ipc";
import { cn } from "@/lib/utils";
import type { RepoSession, SelectedFile } from "@/stores/sessions";
import { useSettingsStore } from "@/stores/settings";

const props = defineProps<{
  session: RepoSession;
}>();

const settings = useSettingsStore();

/** One clickable line in either mode. */
interface FileItem {
  id: string;
  path: string;
  oldPath: string | null;
  letter: string;
  letterClass: string;
  added: number | null;
  deleted: number | null;
  binary: boolean;
  target: DiffTarget;
}

interface Section {
  key: string;
  title: string;
  items: FileItem[];
}

const selectedRow = computed(() => props.session.selectedRow);
const mode = computed<"none" | "commit" | "worktree">(() => {
  const row = selectedRow.value;
  if (!row) return "none";
  return row.kind === "workingTree" ? "worktree" : "commit";
});

// ---- Status letters ------------------------------------------------------------------------

function letterClass(letter: string): string {
  switch (letter.charAt(0)) {
    case "A":
      return "badge-added";
    case "M":
      return "badge-modified";
    case "D":
      return "badge-deleted";
    case "R":
      return "badge-renamed";
    case "C":
      return "badge-renamed";
    case "T":
      return "badge-graphite";
    case "U":
      return "badge-deleted";
    default:
      return "bg-muted text-muted-foreground";
  }
}

function statusLetter(status: ChangeStatus): string {
  switch (status) {
    case "added":
      return "A";
    case "modified":
      return "M";
    case "deleted":
      return "D";
    case "renamed":
      return "R";
    case "copied":
      return "C";
    case "typeChanged":
      return "T";
    case "unmerged":
      return "U";
    case "unknown":
      return "?";
  }
}

// ---- Commit mode ---------------------------------------------------------------------------

const parentIndex = ref(0);
const commitParents = computed(() => (mode.value === "commit" ? (selectedRow.value?.parents ?? []) : []));
const isMerge = computed(() => commitParents.value.length > 1);
/** Parent passed to the backend: only set explicitly for merges, `null` means "first parent". */
const parentOid = computed<string | null>(() =>
  isMerge.value ? (commitParents.value[parentIndex.value] ?? commitParents.value[0] ?? null) : null,
);

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
    message.value = null;
    messageExpanded.value = false;
    messageError.value = null;
  },
);

watch(
  () => [selectedRow.value?.oid, parentOid.value, mode.value] as const,
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

const commitSections = computed<Section[]>(() => {
  const target = commitTarget.value;
  if (!target) return [];
  const items: FileItem[] = changes.value.map((c) => {
    const letter = statusLetter(c.status);
    return {
      id: `commit:${c.path}`,
      path: c.path,
      oldPath: c.oldPath,
      letter,
      letterClass: letterClass(letter),
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

function worktreeItem(entry: StatusEntry, section: string, letter: string, target: DiffTarget): FileItem {
  return {
    id: `${section}:${entry.path}`,
    path: entry.path,
    oldPath: entry.origPath,
    letter,
    letterClass: letterClass(letter),
    added: null,
    deleted: null,
    binary: false,
    target,
  };
}

const worktreeSections = computed<Section[]>(() => {
  const entries = props.session.status?.entries ?? [];
  const staged: FileItem[] = [];
  const unstaged: FileItem[] = [];
  const untracked: FileItem[] = [];
  const conflicted: FileItem[] = [];
  for (const e of entries) {
    if (e.ignored) continue;
    if (e.conflicted) {
      conflicted.push(worktreeItem(e, "conflicted", `${e.x}${e.y}`.replace(/\./g, ""), { type: "unstaged" }));
      continue;
    }
    if (e.untracked) {
      untracked.push(worktreeItem(e, "untracked", "?", { type: "untracked" }));
      continue;
    }
    if (e.staged || e.x !== ".") staged.push(worktreeItem(e, "staged", e.x, { type: "staged" }));
    if (e.unstaged || (e.y !== "." && e.y !== "?")) unstaged.push(worktreeItem(e, "unstaged", e.y, { type: "unstaged" }));
  }
  return [
    { key: "staged", title: "Staged", items: staged },
    { key: "unstaged", title: "Unstaged", items: unstaged },
    { key: "untracked", title: "Untracked", items: untracked },
    { key: "conflicted", title: "Conflicted", items: conflicted },
  ];
});

// ---- Shared list behaviour -----------------------------------------------------------------

const sections = computed<Section[]>(() => {
  if (mode.value === "commit") return commitSections.value;
  if (mode.value === "worktree") return worktreeSections.value;
  return [];
});

const flat = computed<FileItem[]>(() => sections.value.flatMap((s) => s.items));

function isSelected(item: FileItem): boolean {
  const sel = props.session.selectedFile;
  return sel !== null && sel.path === item.path && sel.target.type === item.target.type;
}

function select(item: FileItem): void {
  const next: SelectedFile = { path: item.path, oldPath: item.oldPath, target: item.target };
  props.session.selectedFile = next;
}

// Drop a selected working tree file that is no longer in the list after a status refresh.
watch(flat, (items) => {
  const sel = props.session.selectedFile;
  if (!sel || mode.value === "none") return;
  if (!items.some((i) => i.path === sel.path && i.target.type === sel.target.type)) {
    props.session.selectedFile = null;
  }
});

function onKeydown(event: KeyboardEvent): void {
  if (event.key !== "ArrowDown" && event.key !== "ArrowUp") return;
  const items = flat.value;
  if (items.length === 0) return;
  event.preventDefault();
  const current = items.findIndex((i) => isSelected(i));
  let next: number;
  if (current < 0) next = event.key === "ArrowDown" ? 0 : items.length - 1;
  else next = Math.min(items.length - 1, Math.max(0, current + (event.key === "ArrowDown" ? 1 : -1)));
  const item = items[next];
  if (item) {
    select(item);
    document.getElementById(rowId(item))?.scrollIntoView({ block: "nearest" });
  }
}

function rowId(item: FileItem): string {
  return `file-${props.session.key.slice(0, 8)}-${item.id.replace(/[^a-zA-Z0-9_-]/g, "_")}`;
}

function splitPath(path: string): { dir: string; name: string } {
  const idx = path.lastIndexOf("/");
  return idx < 0 ? { dir: "", name: path } : { dir: path.slice(0, idx + 1), name: path.slice(idx + 1) };
}

const totalCount = computed(() => flat.value.length);
</script>

<template>
  <section class="flex h-full min-h-0 flex-col outline-none" tabindex="0" @keydown="onKeydown">
    <PanelHeader title="Files">
      <span v-if="mode === 'worktree'" class="truncate text-xs text-muted-foreground">Working tree</span>
      <span v-else-if="mode === 'commit' && selectedRow" class="truncate font-mono text-xs text-muted-foreground">
        {{ shortOid(selectedRow.oid) }}
      </span>
      <template #actions>
        <span v-if="mode !== 'none'" class="text-[0.7rem] text-muted-foreground tabular-nums">{{ totalCount }} files</span>
      </template>
    </PanelHeader>

    <EmptyState v-if="mode === 'none'" message="Select a commit or the working tree to list its files." />

    <template v-else>
      <!-- Commit header -->
      <div v-if="mode === 'commit' && selectedRow" class="shrink-0 border-b border-border bg-muted/20 px-2 py-1 text-base">
        <div class="flex items-center gap-2 text-xs text-muted-foreground">
          <span class="font-mono text-foreground" :title="selectedRow.oid">{{ shortOid(selectedRow.oid) }}</span>
          <span class="truncate" :title="`${selectedRow.authorName} <${selectedRow.authorEmail}>`">{{ selectedRow.authorName }}</span>
          <span class="ml-auto shrink-0 tabular-nums" :title="isoTime(selectedRow.authorTime)">{{ dateText }}</span>
        </div>
        <button type="button" class="mt-0.5 flex w-full items-start gap-1 text-left" @click="toggleMessage">
          <component :is="messageExpanded ? ChevronDownIcon : ChevronRightIcon" class="mt-0.5 size-3 shrink-0 text-muted-foreground" />
          <span class="min-w-0 flex-1 font-semibold break-words">{{ selectedRow.subject }}</span>
        </button>
        <div v-if="messageExpanded" class="pl-4 pt-1">
          <p v-if="messageLoading" class="text-xs text-muted-foreground">Loading message…</p>
          <p v-else-if="messageError" class="text-xs text-deleted-fg">{{ messageError }}</p>
          <pre v-else-if="messageBody.length > 0" class="max-h-40 overflow-auto font-sans text-xs whitespace-pre-wrap text-muted-foreground scrollbar-thin">{{ messageBody }}</pre>
          <p v-else class="text-xs text-muted-foreground italic">No further message body.</p>
        </div>
        <div v-if="isMerge" class="mt-1 flex items-center gap-1 text-[0.7rem] text-muted-foreground">
          <span>Compare with</span>
          <button
            v-for="(p, i) in commitParents"
            :key="p"
            type="button"
            :aria-pressed="parentIndex === i"
            :class="
              cn(
                'h-[1.2rem] rounded-full border px-2 leading-none transition-colors',
                parentIndex === i ? 'border-transparent bg-accent text-accent-foreground' : 'border-border hover:text-foreground',
              )
            "
            :title="p"
            @click="parentIndex = i"
          >
            parent {{ i + 1 }}
          </button>
        </div>
      </div>

      <div class="min-h-0 flex-1 overflow-auto scrollbar-thin">
        <div v-if="mode === 'commit' && changesLoading && changes.length === 0" class="flex h-(--row-h) items-center gap-1.5 px-2 text-base text-muted-foreground">
          <LoaderCircleIcon class="size-3 animate-spin" /> Loading changes…
        </div>
        <p v-else-if="changesError" class="px-2 py-1 text-base text-deleted-fg">{{ changesError }}</p>
        <p v-else-if="flat.length === 0" class="px-2 py-2 text-base text-muted-foreground italic">
          {{ mode === "worktree" ? "The working tree is clean." : "No files changed." }}
        </p>

        <template v-for="section in sections" :key="section.key">
          <template v-if="mode === 'commit' || section.items.length > 0">
            <div
              v-if="mode === 'worktree'"
              class="sticky top-0 z-10 flex h-(--row-h) items-center gap-1.5 border-b border-border bg-muted/60 px-2 text-xs font-medium text-muted-foreground backdrop-blur-sm select-none"
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
                  'flex h-(--row-h) w-full cursor-pointer items-center gap-1.5 px-2 text-left text-base',
                  isSelected(item) ? 'bg-accent text-accent-foreground' : 'hover:bg-accent/40',
                )
              "
              :title="item.oldPath ? `${item.oldPath} → ${item.path}` : item.path"
              @click="select(item)"
            >
              <span
                :class="cn('inline-flex h-4 min-w-4 shrink-0 items-center justify-center rounded-sm px-0.5 font-mono text-[0.7rem] leading-none font-semibold', item.letterClass)"
              >
                {{ item.letter }}
              </span>
              <span class="min-w-0 flex-1 truncate">
                <template v-if="item.oldPath && item.oldPath !== item.path">
                  <span class="text-muted-foreground">{{ item.oldPath }}</span>
                  <span class="mx-1 text-muted-foreground">→</span>
                </template>
                <span class="text-muted-foreground">{{ splitPath(item.path).dir }}</span><span>{{ splitPath(item.path).name }}</span>
              </span>
              <span v-if="item.binary" class="shrink-0 text-[0.7rem] text-muted-foreground">binary</span>
              <span v-else-if="item.added !== null || item.deleted !== null" class="shrink-0 font-mono text-[0.7rem] tabular-nums">
                <span class="text-added-fg">+{{ item.added ?? 0 }}</span>
                <span class="ml-1 text-deleted-fg">−{{ item.deleted ?? 0 }}</span>
              </span>
            </button>
          </template>
        </template>
      </div>
    </template>
  </section>
</template>
