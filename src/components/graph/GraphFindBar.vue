<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, ref, watch } from "vue";
import { ChevronDownIcon, ChevronUpIcon, SearchIcon, XIcon } from "@lucide/vue";
import { InputGroup, InputGroupAddon, InputGroupButton, InputGroupInput, InputGroupText } from "@/components/ui/input-group";
import { commands, type SearchDirection } from "@/lib/bindings";
import { errorMessage, IpcError, unwrap } from "@/lib/ipc";
import { useSessionsStore, type RepoSession } from "@/stores/sessions";

/**
 * The Find box over the Commits panel (Ctrl+F, or Edit > Find Commit). It finds commits whose
 * subject, author or oid matches, through `graph_search`: typing selects the first match from
 * the selected row down, Enter the next one and Shift+Enter the previous one, wrapping around
 * the ends. Escape closes it.
 */
const props = defineProps<{
  session: RepoSession;
  /** The row the search starts from: the selected one, or null for the top. */
  current: () => number | null;
}>();

const emit = defineEmits<{
  /** The box closed; the graph takes the keyboard focus back. */
  close: [];
}>();

const sessions = useSessionsStore();
const box = ref<HTMLElement | null>(null);
/** Rows that match the text, from the last search; null before the first. */
const matches = ref<number | null>(null);
let latest = 0;
let typingTimer: ReturnType<typeof setTimeout> | null = null;

const query = computed({
  get: () => props.session.findQuery,
  set: (value: string | number) => {
    props.session.findQuery = String(value);
    scheduleTypingSearch();
  },
});

const summary = computed(() => {
  if (matches.value === null || props.session.findQuery.trim() === "") return "";
  if (matches.value === 0) return "No matches";
  return matches.value === 1 ? "1 match" : `${matches.value.toLocaleString()} matches`;
});

/** Searches from `fromRow` in `direction` and selects the row found. A search started later wins. */
async function search(direction: SearchDirection, fromRow: number | null): Promise<void> {
  const text = props.session.findQuery.trim();
  const ticket = ++latest;
  if (text === "") {
    matches.value = null;
    return;
  }
  // A snapshot swap between the call and its answer makes the version stale; try once more.
  for (let attempt = 0; attempt < 2; attempt += 1) {
    const version = props.session.graph.meta?.version ?? 0;
    if (version === 0) return;
    try {
      const result = await unwrap(commands.graphSearch(props.session.key, version, text, fromRow, direction));
      if (ticket !== latest) return;
      matches.value = result.matches;
      if (result.row !== null) await sessions.selectIndex(props.session.key, version, result.row);
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
    void search("forward", at !== null && at > 0 ? at - 1 : null);
  }, 150);
}

function step(direction: SearchDirection): void {
  if (typingTimer !== null) {
    clearTimeout(typingTimer);
    typingTimer = null;
  }
  void search(direction, props.current());
}

function close(): void {
  props.session.findOpen = false;
  latest += 1;
  emit("close");
}

function onKeydown(event: KeyboardEvent): void {
  if (event.key === "Enter") {
    event.preventDefault();
    step(event.shiftKey ? "backward" : "forward");
  } else if (event.key === "Escape") {
    event.preventDefault();
    close();
  } else if ((event.ctrlKey || event.metaKey) && (event.key === "f" || event.key === "F")) {
    // Ctrl+F again selects the text, and never opens the web view's own find bar.
    event.preventDefault();
    focusInput();
  }
  // The graph's own keys (arrows, Ctrl+C) must not act on the rows while typing here.
  event.stopPropagation();
}

function focusInput(): void {
  void nextTick(() => {
    const input = box.value?.querySelector("input");
    input?.focus();
    input?.select();
  });
}

watch(() => props.session.findFocus, focusInput, { immediate: true });

onBeforeUnmount(() => {
  if (typingTimer !== null) clearTimeout(typingTimer);
});
</script>

<template>
  <div ref="box" class="w-80 max-w-[calc(100%-1.5rem)] rounded-lg bg-popover shadow-md" @keydown="onKeydown">
    <InputGroup>
      <InputGroupAddon>
        <SearchIcon />
      </InputGroupAddon>
      <InputGroupInput v-model="query" placeholder="Find subject, author or commit ID" aria-label="Find commits" spellcheck="false" />
      <InputGroupAddon align="inline-end">
        <InputGroupText v-if="summary" class="text-xs tabular-nums">{{ summary }}</InputGroupText>
        <InputGroupButton size="icon-xs" title="Previous match (Shift+Enter)" aria-label="Previous match" @click="step('backward')">
          <ChevronUpIcon />
        </InputGroupButton>
        <InputGroupButton size="icon-xs" title="Next match (Enter)" aria-label="Next match" @click="step('forward')">
          <ChevronDownIcon />
        </InputGroupButton>
        <InputGroupButton size="icon-xs" title="Close (Escape)" aria-label="Close" @click="close">
          <XIcon />
        </InputGroupButton>
      </InputGroupAddon>
    </InputGroup>
  </div>
</template>
