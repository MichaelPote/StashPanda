<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, ref, watch } from "vue";
import { ChevronLeftIcon, ChevronRightIcon, SearchIcon, XIcon } from "@lucide/vue";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { commands, type SearchDirection } from "@/lib/bindings";
import { errorMessage, IpcError, unwrap } from "@/lib/ipc";
import { vPixelSnap } from "@/lib/pixelSnap";
import { useSessionsStore, type RepoSession } from "@/stores/sessions";

/**
 * The search box in the Commits panel's header. It finds commits whose subject, author or
 * commit ID matches, through `graph_search`, which also lists every matching row. It searches
 * from `MIN_CHARS` characters on: one or two match so much that marking them all slows typing
 * down, and say little about what is wanted. Typing marks
 * the matches in the list with pills (`session.findText`) and selects the first one from the
 * selected row down; < and > (or Shift+Enter and Enter) select the previous and next match,
 * wrapping around the ends. Escape clears it. Ctrl+F and Edit > Find Commit put the cursor here
 * (`session.findFocus`).
 */
const props = defineProps<{
  session: RepoSession;
  /** The selected row, where searching and stepping start; null for the top. */
  current: () => number | null;
}>();

const emit = defineEmits<{
  /** Escape: the commit list takes the keyboard focus back. */
  close: [];
}>();

const MIN_CHARS = 3;

const sessions = useSessionsStore();
const input = ref<HTMLInputElement | null>(null);
/** Every matching row from the last search, and the graph version they belong to; null before one. */
const found = ref<{ rows: number[]; version: number } | null>(null);
let latest = 0;
let typingTimer: ReturnType<typeof setTimeout> | null = null;

const query = computed({
  get: () => props.session.findQuery,
  set: (value: string) => {
    props.session.findQuery = value;
    scheduleTypingSearch();
  },
});

/** "3 / 12" while a match is selected, "12" otherwise, "0" for none; empty before a search. */
const counter = computed(() => {
  const f = found.value;
  if (!f || props.session.findText === "") return "";
  const at = props.current();
  const i = at === null ? -1 : f.rows.indexOf(at);
  return i >= 0 ? `${i + 1} / ${f.rows.length}` : String(f.rows.length);
});

/**
 * Searches for the box's text. With `select`, selects the match `graph_search` picks from
 * `fromRow` in `direction`; without, only refreshes the matches (the graph changed under them).
 * A search started later wins.
 */
async function search(direction: SearchDirection, fromRow: number | null, select: boolean): Promise<void> {
  const text = props.session.findQuery.trim();
  const ticket = ++latest;
  if (text.length < MIN_CHARS) {
    found.value = null;
    props.session.findText = "";
    return;
  }
  // A snapshot swap between the call and its answer makes the version stale; try once more.
  for (let attempt = 0; attempt < 2; attempt += 1) {
    const version = props.session.graph.meta?.version ?? 0;
    if (version === 0) return;
    try {
      const result = await unwrap(commands.graphSearch(props.session.key, version, text, fromRow, direction));
      if (ticket !== latest) return;
      found.value = { rows: result.rows, version };
      props.session.findText = text;
      if (select && result.row !== null) await sessions.selectIndex(props.session.key, version, result.row);
      return;
    } catch (e) {
      if (e instanceof IpcError && e.error.kind === "staleVersion") continue;
      sessions.notify(errorMessage(e));
      return;
    }
  }
}

/** While typing, the selected row itself counts, so a match stays put as the text grows. */
function scheduleTypingSearch(): void {
  if (typingTimer !== null) clearTimeout(typingTimer);
  typingTimer = setTimeout(() => {
    typingTimer = null;
    const at = props.current();
    void search("forward", at !== null && at > 0 ? at - 1 : null, true);
  }, 200);
}

/** The previous or next match from the selected row, wrapping; asks the backend when the matches are out of date. */
function step(direction: SearchDirection): void {
  const pending = typingTimer !== null;
  if (typingTimer !== null) {
    clearTimeout(typingTimer);
    typingTimer = null;
  }
  const f = found.value;
  const version = props.session.graph.meta?.version ?? 0;
  if (pending || !f || f.version !== version || props.session.findText !== props.session.findQuery.trim()) {
    void search(direction, props.current(), true);
    return;
  }
  if (f.rows.length === 0) return;
  const at = props.current();
  const row =
    direction === "forward"
      ? (f.rows.find((r) => at === null || r > at) ?? f.rows[0])
      : ([...f.rows].reverse().find((r) => at === null || r < at) ?? f.rows[f.rows.length - 1]);
  void sessions.selectIndex(props.session.key, version, row);
}

function clear(): void {
  if (typingTimer !== null) clearTimeout(typingTimer);
  typingTimer = null;
  latest += 1;
  props.session.findQuery = "";
  props.session.findText = "";
  found.value = null;
}

function onKeydown(event: KeyboardEvent): void {
  if (event.key === "Enter") {
    event.preventDefault();
    step(event.shiftKey ? "backward" : "forward");
  } else if (event.key === "Escape") {
    event.preventDefault();
    clear();
    emit("close");
  } else if ((event.ctrlKey || event.metaKey) && (event.key === "f" || event.key === "F")) {
    // Ctrl+F again selects the text, and never opens the web view's own find bar.
    event.preventDefault();
    focusInput();
  }
  // The commit list's own keys (arrows, Ctrl+C) must not act on the rows while typing here.
  event.stopPropagation();
}

function focusInput(): void {
  void nextTick(() => {
    input.value?.focus();
    input.value?.select();
  });
}

watch(() => props.session.findFocus, focusInput);

// A new graph (a fetch, a commit, a refresh) can move or drop the matches: find them again,
// without moving the selection.
watch(
  () => props.session.graph.meta?.version,
  (version) => {
    if (version && found.value && found.value.version !== version) void search("forward", null, false);
  },
);

// A tab reopened with text still in its box finds the matches again once its graph is there.
if (props.session.findQuery.trim().length >= MIN_CHARS) void search("forward", null, false);

onBeforeUnmount(() => {
  if (typingTimer !== null) clearTimeout(typingTimer);
});

// 17px tall, like the box: the header has 23px under its border, so 3px above and below. The
// 12px icon goes 2px from the top, on a whole pixel, rather than centred on a half one.
const BUTTON_CLASS =
  "inline-flex h-[17px] w-[18px] shrink-0 items-start justify-center rounded-sm pt-[2px] text-muted-foreground hover:bg-ghost-hover hover:text-foreground disabled:pointer-events-none disabled:opacity-40";
</script>

<template>
  <!-- On whole pixels, so the 12px icons of < and > are sharp (see index.css): whole-pixel gaps,
       and v-pixel-snap, since the box sits against the pane's right edge, which can fall
       part-way through a pixel. -->
  <div v-pixel-snap class="flex items-center gap-[2px]" @keydown="onKeydown" @mousedown.stop @click.stop @contextmenu.stop>
    <div class="well flex h-[17px] w-48 items-center gap-1 rounded-md border pr-[2px] pl-[4px]">
      <SearchIcon class="size-3 shrink-0 text-muted-foreground" aria-hidden="true" />
      <input
        ref="input"
        v-model="query"
        type="text"
        class="min-w-0 flex-1 bg-transparent text-[0.7rem] text-foreground outline-none placeholder:text-muted-foreground"
        placeholder="Search commits"
        aria-label="Search commits by subject, author or commit ID"
        spellcheck="false"
        autocomplete="off"
      />
      <span v-if="counter" class="shrink-0 text-[0.7rem] text-muted-foreground tabular-nums">{{ counter }}</span>
      <Tooltip v-if="session.findQuery !== ''">
        <TooltipTrigger as-child>
          <button type="button" class="inline-flex size-[14px] shrink-0 items-center justify-center rounded-sm text-muted-foreground hover:text-foreground" aria-label="Clear the search" @click="clear">
            <XIcon class="size-3" />
          </button>
        </TooltipTrigger>
        <TooltipContent side="bottom">Clear the search (Escape)</TooltipContent>
      </Tooltip>
    </div>
    <Tooltip>
      <TooltipTrigger as-child>
        <button type="button" :class="BUTTON_CLASS" aria-label="Previous match" :disabled="!found || found.rows.length === 0" @click="step('backward')">
          <ChevronLeftIcon class="size-3.5" />
        </button>
      </TooltipTrigger>
      <TooltipContent side="bottom">Previous match (Shift+Enter)</TooltipContent>
    </Tooltip>
    <Tooltip>
      <TooltipTrigger as-child>
        <button type="button" :class="BUTTON_CLASS" aria-label="Next match" :disabled="!found || found.rows.length === 0" @click="step('forward')">
          <ChevronRightIcon class="size-3.5" />
        </button>
      </TooltipTrigger>
      <TooltipContent side="bottom">Next match (Enter)</TooltipContent>
    </Tooltip>
  </div>
</template>
