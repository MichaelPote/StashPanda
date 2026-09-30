<script setup lang="ts">
import { computed, nextTick, ref, watch } from "vue";
import { LoaderCircleIcon, TriangleAlertIcon } from "@lucide/vue";
import { commands } from "@/lib/bindings";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Label } from "@/components/ui/label";
import { Textarea } from "@/components/ui/textarea";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { STATE_CLASSES, commitPaths, splitPath } from "@/lib/files";
import { errorMessage, unwrap } from "@/lib/ipc";
import { opFailureText, runOp } from "@/lib/repoActions";
import { cn } from "@/lib/utils";
import { useEventListener } from "@vueuse/core";
import { useListSelection, type ListSelectionState } from "@/composables/useListSelection";
import { isEditableTarget, isSelectAllKey } from "@/lib/files";
import { useDialogsStore } from "@/stores/dialogs";
import { emptyFileSelection, useSessionsStore } from "@/stores/sessions";

const dialogs = useDialogsStore();
const sessions = useSessionsStore();

const request = computed(() => dialogs.current("commit"));
const session = computed(() => {
  const r = request.value;
  return r ? (sessions.sessions.get(r.key) ?? null) : null;
});

const message = ref("");
/** Ticked paths. Replaced, never mutated. */
const ticked = ref<Set<string>>(new Set());
const amend = ref(false);
const error = ref<string | null>(null);
const submitting = ref(false);
const loadingPrevious = ref(false);
/** The previous commit's message as loaded for an amend, so unticking Amend can take it back out. */
let previousMessage: string | null = null;
const messageBox = ref<InstanceType<typeof Textarea> | null>(null);

watch(request, (r) => {
  if (!r) return;
  message.value = session.value?.commitDraft ?? "";
  ticked.value = new Set(r.ticked);
  rows.value = { ids: new Set(), anchorId: null };
  list.cursorId.value = null;
  amend.value = false;
  previousMessage = null;
  error.value = null;
  submitting.value = false;
  loadingPrevious.value = false;
});

// Keep the draft on the session, so Cancel and reopening brings the text back.
watch(message, (text) => {
  const s = session.value;
  if (s && !amend.value) s.commitDraft = text;
});

function focusMessage(event: Event): void {
  // Focus the message rather than the first button, which is what the dialog would pick.
  event.preventDefault();
  void nextTick(() => (messageBox.value?.$el as HTMLTextAreaElement | undefined)?.focus());
}

async function onAmendChange(): Promise<void> {
  const r = request.value;
  if (!r) return;
  if (!amend.value) {
    if (previousMessage !== null && message.value === previousMessage) message.value = session.value?.commitDraft ?? "";
    previousMessage = null;
    return;
  }
  if (message.value.trim().length > 0) return;
  loadingPrevious.value = true;
  try {
    const text = (await unwrap(commands.getCommitMessage(r.key, "HEAD"))).replace(/\r\n/g, "\n").trimEnd();
    if (amend.value && message.value.trim().length === 0) {
      previousMessage = text;
      message.value = text;
    }
  } catch (e) {
    error.value = errorMessage(e);
  } finally {
    loadingPrevious.value = false;
  }
}

const files = computed(() => request.value?.files ?? []);
const tickedFiles = computed(() => files.value.filter((f) => ticked.value.has(f.path)));
/** Null commits the index as it is; see `commitPaths`. */
const paths = computed(() => commitPaths(files.value, ticked.value));
/** Ticked files whose unstaged changes are committed too, because the commit names paths. */
const partlyStaged = computed(() => (paths.value === null ? [] : tickedFiles.value.filter((f) => f.staged && f.unstaged)));
const conflicted = computed(() => tickedFiles.value.filter((f) => f.state === "conflict"));

const allTicked = computed(() => files.value.length > 0 && tickedFiles.value.length === files.value.length);
const someTicked = computed(() => tickedFiles.value.length > 0 && !allTicked.value);

function toggleFile(path: string): void {
  const next = new Set(ticked.value);
  if (next.has(path)) next.delete(path);
  else next.add(path);
  ticked.value = next;
}

// ---- Row selection (separate from the ticks) ------------------------------------------------

/** Highlighted rows in the list. Space flips the ticks of these. */
const rows = ref<ListSelectionState>({ ids: new Set(), anchorId: null });
const listEl = ref<HTMLElement | null>(null);

function rowId(path: string): string {
  return `commit-file-${encodeURIComponent(path).replace(/[^a-zA-Z0-9_-]/g, "_")}`;
}

const list = useListSelection({
  order: () => files.value.map((f) => f.path),
  get: () => rows.value,
  set: (next) => {
    rows.value = next;
  },
  reveal: (path) => document.getElementById(rowId(path))?.scrollIntoView({ block: "nearest" }),
});

/**
 * Space flips the selected rows' ticks together, to the opposite of the cursor row's tick, so a
 * mixed selection ends up all ticked or all unticked. With nothing selected it flips the cursor row.
 */
function flipSelectedTicks(): void {
  const cursor = list.cursor();
  const selected = files.value.map((f) => f.path).filter((p) => rows.value.ids.has(p));
  const targets = selected.length > 0 ? selected : cursor !== null ? [cursor] : [];
  if (targets.length === 0) return;
  const reference = cursor !== null && targets.includes(cursor) ? cursor : targets[0];
  const tick = reference === undefined || !ticked.value.has(reference);
  const next = new Set(ticked.value);
  for (const p of targets) {
    if (tick) next.add(p);
    else next.delete(p);
  }
  ticked.value = next;
}

function onListKeydown(event: KeyboardEvent): void {
  if (event.key === " " && !event.ctrlKey && !event.metaKey && !event.altKey) {
    event.preventDefault();
    flipSelectedTicks();
    return;
  }
  list.onKeydown(event);
}

// Ctrl+A anywhere in the dialog outside a text field selects every row, like it does in the
// Files panel. App.vue has already stopped the browser from selecting the page text.
useEventListener(window, "keydown", (event: KeyboardEvent) => {
  if (request.value === null || !isSelectAllKey(event) || isEditableTarget(event.target)) return;
  if (listEl.value?.contains(event.target as Node)) return; // the list's own handler does it
  list.selectAll();
});

function toggleAll(): void {
  ticked.value = allTicked.value ? new Set() : new Set(files.value.map((f) => f.path));
}

/** What the tick marks are, in one line above the list. */
const listCaption = computed(() => {
  const n = files.value.length;
  if (n === 0) return "No changes";
  return `${tickedFiles.value.length} of ${n} ticked`;
});

const hasMessage = computed(() => message.value.trim().length > 0);
/** An amend can reword the last commit without adding files. */
const canCommit = computed(
  () => request.value !== null && !submitting.value && !loadingPrevious.value && (tickedFiles.value.length > 0 || amend.value) && (hasMessage.value || amend.value),
);
const commitHint = computed(() => {
  if (files.value.length === 0 && !amend.value) return "There is nothing to commit.";
  if (tickedFiles.value.length === 0 && !amend.value) return "Tick the files to commit.";
  if (!hasMessage.value && !amend.value) return "Type a commit message.";
  return null;
});

/** Button labels say what will happen: an amend with nothing ticked only rewrites the message. */
const commitLabel = computed(() => {
  if (!amend.value) return "Commit";
  return tickedFiles.value.length === 0 ? "Amend Message" : "Amend Commit";
});
const pushLabel = computed(() => (amend.value ? "Amend & Push" : "Commit & Push"));

function onOpenChange(open: boolean): void {
  if (!open) dialogs.cancel();
}

function onMessageKeydown(event: KeyboardEvent): void {
  if (event.key === "Enter" && (event.ctrlKey || event.metaKey)) {
    event.preventDefault();
    void submit();
  }
}

async function submit(): Promise<void> {
  const r = request.value;
  if (!r || !canCommit.value) return;
  // An amend with nothing ticked passes an empty list, which changes only the message; null
  // would commit whatever is staged.
  const commitWith = tickedFiles.value.length === 0 ? [] : paths.value;
  const n = tickedFiles.value.length;
  submitting.value = true;
  error.value = null;
  try {
    const result = await runOp(r.key, commands.opCommit(r.key, message.value, commitWith, amend.value, false));
    if (!result.ok) {
      error.value = opFailureText(result, amend.value ? "Amending the commit" : "Committing");
      return;
    }
    const s = session.value;
    if (s) {
      s.commitDraft = "";
      s.fileSelection = emptyFileSelection();
    }
    const what = n === 0 ? "the message" : `${n} ${n === 1 ? "file" : "files"}`;
    const where = r.branch ? ` on ${r.branch}` : "";
    sessions.notify(amend.value ? `Amended the last commit${where} with ${what}.` : `Committed ${what}${where}.`);
    dialogs.resolve("commit", result);
  } catch (e) {
    error.value = errorMessage(e);
  } finally {
    submitting.value = false;
  }
}
</script>

<template>
  <Dialog :open="request !== null" @update:open="onOpenChange">
    <DialogContent v-if="request" class="sm:max-w-xl" @open-auto-focus="focusMessage">
      <DialogHeader>
        <DialogTitle>{{ amend ? "Amend last commit" : "Commit" }}</DialogTitle>
        <DialogDescription>
          <template v-if="request.branch">To branch <span class="font-mono text-foreground">{{ request.branch }}</span>.</template>
          <template v-else-if="request.hasHead">HEAD is detached; the commit will not be on any branch.</template>
          <template v-else>This is the first commit in the repository.</template>
        </DialogDescription>
      </DialogHeader>

      <form class="grid min-w-0 gap-3" @submit.prevent="submit">
        <div class="grid min-w-0 gap-1.5">
          <div class="flex items-center gap-2">
            <label class="flex items-center gap-2 text-base font-medium select-none">
              <input
                type="checkbox"
                class="size-3.5 accent-primary"
                :checked="allTicked"
                :indeterminate="someTicked"
                :disabled="files.length === 0"
                aria-label="Tick every file"
                @change="toggleAll"
              />
              Files
            </label>
            <span class="text-xs text-muted-foreground">{{ listCaption }}</span>
          </div>
          <p v-if="request.fromSelection" class="text-xs text-muted-foreground">Ticked from your selection in the file list.</p>
          <ul
            ref="listEl"
            role="listbox"
            aria-multiselectable="true"
            aria-label="Files to commit"
            tabindex="0"
            class="max-h-56 min-h-(--row-h) overflow-auto rounded-lg border border-border well py-0.5 outline-none scrollbar-thin focus-visible:ring-2 focus-visible:ring-ring/50"
            @keydown="onListKeydown"
          >
            <li v-if="files.length === 0" class="flex h-(--row-h) items-center px-2 text-base text-muted-foreground italic">
              {{ amend ? "No files; only the message changes." : "Nothing to commit." }}
            </li>
            <li
              v-for="file in files"
              :id="rowId(file.path)"
              :key="file.path"
              role="option"
              :aria-selected="list.isSelected(file.path)"
              :class="
                cn(
                  'flex h-(--row-h) min-w-0 cursor-default items-center gap-1.5 px-2 text-base select-none',
                  list.isSelected(file.path) ? 'row-selected' : 'hover:bg-row-hover',
                  list.isCursor(file.path) && 'shadow-[inset_2px_0_0_var(--ring)]',
                )
              "
              :title="file.oldPath && file.oldPath !== file.path ? `${file.oldPath} → ${file.path}` : file.path"
              @mousedown="list.onMousedown"
              @click="list.onClick(file.path, $event)"
            >
              <!-- The checkbox flips only its own file; tabindex -1 keeps Tab moving past the list. -->
              <input
                type="checkbox"
                tabindex="-1"
                class="size-3.5 shrink-0 accent-primary"
                :checked="ticked.has(file.path)"
                :aria-label="`Include ${file.path}`"
                @click.stop
                @change="toggleFile(file.path)"
              />
              <span
                :class="cn('inline-flex h-4 min-w-4 shrink-0 items-center justify-center rounded-sm px-0.5 font-mono text-[0.7rem] leading-none font-semibold', STATE_CLASSES[file.state].badge)"
              >
                {{ file.letter }}
              </span>
              <span :class="cn('min-w-0 flex-1 truncate', !ticked.has(file.path) && 'opacity-55')">
                <template v-if="file.oldPath && file.oldPath !== file.path">
                  <span class="text-muted-foreground">{{ file.oldPath }}</span>
                  <span class="mx-1 text-muted-foreground">→</span>
                </template>
                <span class="text-muted-foreground">{{ splitPath(file.path).dir }}</span><span>{{ splitPath(file.path).name }}</span>
              </span>
              <span v-if="file.staged && file.unstaged" class="shrink-0 text-[0.7rem] text-muted-foreground">partly staged</span>
              <span v-else-if="file.staged" class="shrink-0 text-[0.7rem] text-muted-foreground">staged</span>
            </li>
          </ul>
          <p v-if="partlyStaged.length > 0" class="flex items-start gap-1.5 text-xs text-modified-fg">
            <TriangleAlertIcon class="mt-px size-3.5 shrink-0" />
            <span>
              {{ partlyStaged.length === 1 ? partlyStaged[0]?.path + " also has" : partlyStaged.length + " files also have" }}
              changes that are not staged. Because the ticked files are not exactly the staged ones, each is committed as it is on disk, so those changes are included.
            </span>
          </p>
          <p v-if="conflicted.length > 0" class="flex items-start gap-1.5 text-xs text-modified-fg">
            <TriangleAlertIcon class="mt-px size-3.5 shrink-0" />
            <span>Committing a conflicted file marks its conflict as resolved, with the file as it is on disk.</span>
          </p>
        </div>

        <div class="grid gap-1.5">
          <Label for="commit-message" class="text-base">Message</Label>
          <Textarea
            id="commit-message"
            ref="messageBox"
            v-model="message"
            class="h-36 resize-y font-mono text-base field-sizing-fixed"
            placeholder="Summary on the first line&#10;&#10;Details after a blank line"
            spellcheck="true"
            :disabled="loadingPrevious"
            @keydown="onMessageKeydown"
          />
          <div class="flex items-center gap-3">
            <label class="flex items-center gap-2 text-base select-none" :class="!request.hasHead && 'opacity-50'">
              <input v-model="amend" type="checkbox" class="size-3.5 accent-primary" :disabled="!request.hasHead" @change="onAmendChange" />
              Amend the last commit
            </label>
            <LoaderCircleIcon v-if="loadingPrevious" class="size-3.5 animate-spin text-muted-foreground" />
            <span class="ml-auto text-xs text-muted-foreground">Ctrl+Enter to confirm. Space ticks or unticks the selected files.</span>
          </div>
        </div>

        <p v-if="error" class="rounded-md border border-destructive/40 bg-destructive/10 px-2.5 py-1.5 text-base break-words whitespace-pre-wrap text-destructive">
          {{ error }}
        </p>
      </form>

      <DialogFooter class="items-center">
        <span v-if="commitHint" class="mr-auto text-xs text-muted-foreground">{{ commitHint }}</span>
        <Button type="button" variant="outline" size="sm" @click="dialogs.cancel()">Cancel</Button>
        <Tooltip>
          <TooltipTrigger as-child>
            <!-- A disabled button swallows pointer events, so the wrapper span carries the tooltip. -->
            <span class="inline-flex">
              <Button type="button" variant="outline" size="sm" disabled>{{ pushLabel }}</Button>
            </span>
          </TooltipTrigger>
          <TooltipContent side="top">Push is not built yet (milestone 3).</TooltipContent>
        </Tooltip>
        <Button type="button" size="sm" :disabled="!canCommit" @click="submit">
          <LoaderCircleIcon v-if="submitting" class="animate-spin" />
          {{ commitLabel }}
        </Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
</template>
