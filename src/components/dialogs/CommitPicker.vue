<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from "vue";
import { ChevronsUpDownIcon, LoaderCircleIcon } from "@lucide/vue";
import { ListboxContent, ListboxFilter, ListboxItem, ListboxRoot } from "reka-ui";
import { commands, type GraphRow } from "@/lib/bindings";
import { Button } from "@/components/ui/button";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { formatRelativeTime } from "@/lib/format";
import { errorMessage, IpcError, unwrap } from "@/lib/ipc";
import { shortSha } from "@/lib/refNames";
import { useSessionsStore } from "@/stores/sessions";

/**
 * A dropdown of the commits in the graph with a search box: the newest first, or those whose
 * subject, author or sha matches what is typed, as the graph's Find box matches them. Commits
 * only on branches hidden in the graph are not in it.
 */
const props = defineProps<{
  sessionKey: string;
  /** For a `<Label for>`. */
  id?: string;
}>();

const chosen = defineModel<GraphRow | null>({ required: true });

/** Most commits listed at once; the search narrows it down. */
const LIMIT = 200;

const sessions = useSessionsStore();
const open = ref(false);
const query = ref("");
const rows = ref<GraphRow[]>([]);
/** How many commits match the search, when it has more than are listed; null without a search. */
const matches = ref<number | null>(null);
const loading = ref(false);
const error = ref<string | null>(null);

let latest = 0;
let timer: ReturnType<typeof setTimeout> | null = null;

/** Lists the commits for `text`. A load started later wins; a graph rebuilt meanwhile is asked again once. */
async function load(text: string): Promise<void> {
  const ticket = ++latest;
  loading.value = true;
  error.value = null;
  try {
    for (let attempt = 0; attempt < 2; attempt += 1) {
      const version = sessions.sessions.get(props.sessionKey)?.graph.meta?.version ?? 0;
      if (version === 0) {
        error.value = "The history is still loading.";
        return;
      }
      try {
        let found: GraphRow[];
        let count: number | null = null;
        if (text.length === 0) {
          found = await unwrap(commands.graphRows(props.sessionKey, version, 0, LIMIT + 1));
        } else {
          const search = await unwrap(commands.graphSearch(props.sessionKey, version, text, null, "forward"));
          found = await unwrap(commands.graphRowsAt(props.sessionKey, version, search.rows.slice(0, LIMIT + 1)));
          count = search.matches;
        }
        if (ticket !== latest) return;
        rows.value = found.filter((r) => r.kind === "commit").slice(0, LIMIT);
        matches.value = count;
        return;
      } catch (e) {
        if (e instanceof IpcError && e.error.kind === "staleVersion") continue;
        throw e;
      }
    }
  } catch (e) {
    if (ticket === latest) error.value = errorMessage(e);
  } finally {
    if (ticket === latest) loading.value = false;
  }
}

watch(open, (isOpen) => {
  if (isOpen) void load(query.value.trim());
});
watch(query, (text) => {
  if (timer !== null) clearTimeout(timer);
  timer = setTimeout(() => {
    timer = null;
    void load(text.trim());
  }, 150);
});
onBeforeUnmount(() => {
  if (timer !== null) clearTimeout(timer);
});

function pick(oid: unknown): void {
  const row = rows.value.find((r) => r.oid === oid);
  if (!row) return;
  chosen.value = row;
  open.value = false;
}

/** The names on a commit, for telling commits apart in the list. */
function names(row: GraphRow): string {
  return row.refs.map((r) => r.short).join(", ");
}

const moreThanListed = computed(() => (matches.value !== null && matches.value > rows.value.length ? matches.value : null));
</script>

<template>
  <Popover v-model:open="open">
    <PopoverTrigger as-child>
      <Button :id="id" type="button" variant="outline" class="h-auto min-h-8 w-full justify-start py-1 text-left text-base font-normal" data-commit-picker>
        <template v-if="chosen">
          <span class="shrink-0 font-mono text-muted-foreground">{{ shortSha(chosen.oid) }}</span>
          <span class="min-w-0 flex-1 truncate">{{ chosen.subject }}</span>
        </template>
        <span v-else class="flex-1 text-muted-foreground">Choose a commit…</span>
        <ChevronsUpDownIcon class="ml-auto size-3.5 shrink-0 opacity-60" />
      </Button>
    </PopoverTrigger>
    <PopoverContent align="start" class="w-(--reka-popover-trigger-width) min-w-96 gap-0 p-1">
      <ListboxRoot :model-value="chosen?.oid" highlight-on-hover @update:model-value="pick">
        <div class="relative p-1">
          <ListboxFilter
            v-model="query"
            auto-focus
            placeholder="Search by message, author or sha"
            class="h-8 w-full rounded-md border border-input bg-transparent px-2 pr-7 text-base outline-hidden focus-visible:border-ring"
            data-commit-picker-search
          />
          <LoaderCircleIcon v-if="loading" class="absolute top-1/2 right-3 size-3.5 -translate-y-1/2 animate-spin text-muted-foreground" />
        </div>
        <p v-if="error" class="px-2 py-1.5 text-xs text-destructive">{{ error }}</p>
        <p v-else-if="!loading && rows.length === 0" class="px-2 py-1.5 text-xs text-muted-foreground">No commit matches.</p>
        <ListboxContent class="max-h-72 overflow-y-auto scrollbar-thin">
          <ListboxItem
            v-for="row in rows"
            :key="row.oid"
            :value="row.oid"
            class="flex cursor-default items-center gap-2 rounded-sm px-2 py-1 text-base outline-hidden select-none data-highlighted:bg-muted data-[state=checked]:font-medium"
            data-commit-picker-item
          >
            <span class="shrink-0 font-mono text-muted-foreground">{{ shortSha(row.oid) }}</span>
            <span class="min-w-0 flex-1 truncate">
              {{ row.subject }}
              <span v-if="row.refs.length > 0" class="ml-1 font-mono text-xs text-muted-foreground">{{ names(row) }}</span>
            </span>
            <span class="shrink-0 text-xs text-muted-foreground">{{ row.authorName }} · {{ formatRelativeTime(row.authorTime) }}</span>
          </ListboxItem>
        </ListboxContent>
        <p v-if="moreThanListed" class="px-2 pt-1 text-xs text-muted-foreground">
          Showing the first {{ rows.length }} of {{ moreThanListed }} matches. Type more to narrow it down.
        </p>
        <p v-else-if="matches === null && rows.length === LIMIT" class="px-2 pt-1 text-xs text-muted-foreground">
          The newest {{ LIMIT }} commits. Search to find an older one.
        </p>
      </ListboxRoot>
    </PopoverContent>
  </Popover>
</template>
