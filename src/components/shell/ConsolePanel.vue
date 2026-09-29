<script setup lang="ts">
import { computed, nextTick, ref, watch } from "vue";
import { useLocalStorage } from "@vueuse/core";
import {
  ChevronDownIcon,
  ChevronRightIcon,
  LoaderCircleIcon,
  PanelBottomCloseIcon,
  PanelBottomOpenIcon,
  Trash2Icon,
} from "@lucide/vue";
import type { Category } from "@/lib/bindings";
import { Button } from "@/components/ui/button";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { formatClockTime, formatDurationMs } from "@/lib/format";
import { GitErrorMessage } from "@/lib/ipc";
import { cn } from "@/lib/utils";
import type { ConsoleEntry, RepoSession } from "@/stores/sessions";
import { useUiStateStore } from "@/stores/uiState";

const props = defineProps<{
  session: RepoSession | null;
  collapsed: boolean;
}>();

const uiState = useUiStateStore();

const CATEGORIES: { id: Category; label: string }[] = [
  { id: "userAction", label: "User" },
  { id: "internal", label: "Internal" },
  { id: "watcher", label: "Watcher" },
];

const filters = useLocalStorage<Record<Category, boolean>>("stashpanda.console.filters", {
  userAction: true,
  internal: true,
  watcher: true,
});

function toggleFilter(id: Category): void {
  filters.value = { ...filters.value, [id]: !filters.value[id] };
}

const entries = computed<ConsoleEntry[]>(() => {
  const all = props.session?.consoleEntries ?? [];
  return all.filter((e) => filters.value[e.category] !== false);
});

const expanded = ref(new Set<number>());

function toggleExpanded(seq: number): void {
  const next = new Set(expanded.value);
  if (next.has(seq)) next.delete(seq);
  else next.add(seq);
  expanded.value = next;
}

function clear(): void {
  props.session?.consoleEntries.splice(0);
  expanded.value = new Set();
}

const scroller = ref<HTMLDivElement | null>(null);
const atBottom = ref(true);

function onScroll(): void {
  const el = scroller.value;
  if (!el) return;
  atBottom.value = el.scrollHeight - el.scrollTop - el.clientHeight < 8;
}

function scrollToBottom(): void {
  const el = scroller.value;
  if (el) el.scrollTop = el.scrollHeight;
}

watch(
  () => entries.value.length,
  async () => {
    if (!atBottom.value) return;
    await nextTick();
    scrollToBottom();
  },
);

watch(
  () => props.session?.key,
  async () => {
    expanded.value = new Set();
    atBottom.value = true;
    await nextTick();
    scrollToBottom();
  },
);

function exitBadgeClass(entry: ConsoleEntry): string {
  if (entry.running) return "border-border text-muted-foreground";
  if (entry.exitCode === 0) return "border-transparent bg-neutral text-foreground";
  return "border-destructive/40 bg-destructive/15 text-destructive";
}

function categoryClass(category: Category): string {
  switch (category) {
    case "userAction":
      return "bg-violet/28 text-primary-tint";
    case "internal":
      return "bg-muted text-muted-foreground";
    case "watcher":
      return "bg-cyan/15 text-cyan";
  }
}

function categoryShort(category: Category): string {
  switch (category) {
    case "userAction":
      return "user";
    case "internal":
      return "internal";
    case "watcher":
      return "watcher";
  }
}
</script>

<template>
  <section class="area flex h-full min-h-0 flex-col">
    <div
      :class="cn('flex shrink-0 items-center gap-2 chrome px-2 select-none', collapsed ? 'h-[26px]' : 'h-[24px] border-b border-border')"
    >
      <span class="smallcaps">Console</span>
      <span v-if="session" class="truncate text-xs text-muted-foreground">
        {{ session.info.displayName }}
      </span>

      <div v-if="!collapsed" class="ml-2 flex items-center gap-1">
        <button
          v-for="c in CATEGORIES"
          :key="c.id"
          type="button"
          :aria-pressed="filters[c.id] !== false"
          class="toggle h-[18px] rounded-sm border px-2 text-[0.7rem] leading-none font-medium transition-colors"
          @click="toggleFilter(c.id)"
        >
          {{ c.label }}
        </button>
      </div>

      <span v-if="!collapsed && session" class="ml-auto text-[0.7rem] text-muted-foreground tabular-nums">
        {{ entries.length }} of {{ session.consoleEntries.length }}
      </span>

      <div :class="cn('flex items-center gap-0.5', collapsed && 'ml-auto')">
        <Tooltip v-if="!collapsed && session">
          <TooltipTrigger as-child>
            <Button variant="ghost" size="icon-xs" class="size-5" @click="clear">
              <Trash2Icon />
              <span class="sr-only">Clear console</span>
            </Button>
          </TooltipTrigger>
          <TooltipContent side="top">Clear console</TooltipContent>
        </Tooltip>
        <Tooltip>
          <TooltipTrigger as-child>
            <Button
              variant="ghost"
              size="icon-xs"
              class="size-5"
              @click="uiState.setConsoleCollapsed(!collapsed)"
            >
              <PanelBottomOpenIcon v-if="collapsed" />
              <PanelBottomCloseIcon v-else />
              <span class="sr-only">{{ collapsed ? "Show console" : "Hide console" }}</span>
            </Button>
          </TooltipTrigger>
          <TooltipContent side="top">{{ collapsed ? "Show console" : "Hide console" }}</TooltipContent>
        </Tooltip>
      </div>
    </div>

    <template v-if="!collapsed">
      <div
        v-if="!session"
        class="flex min-h-0 flex-1 items-center justify-center text-base text-muted-foreground"
      >
        No repository open
      </div>
      <div
        v-else-if="entries.length === 0"
        class="flex min-h-0 flex-1 items-center justify-center text-base text-muted-foreground"
      >
        {{ session.consoleEntries.length === 0 ? "No Git commands have run yet." : "All entries are hidden by the filters." }}
      </div>
      <div
        v-else
        ref="scroller"
        class="min-h-0 flex-1 overflow-auto font-mono text-xs scrollbar-thin"
        @scroll.passive="onScroll"
      >
        <div v-for="entry in entries" :key="entry.seq" class="border-b border-border/60">
          <button
            type="button"
            class="flex h-(--row-h) w-full items-center gap-2 px-2 text-left hover:bg-row-hover"
            @click="toggleExpanded(entry.seq)"
          >
            <component
              :is="expanded.has(entry.seq) ? ChevronDownIcon : ChevronRightIcon"
              class="size-3 shrink-0 text-muted-foreground"
            />
            <span class="shrink-0 text-muted-foreground tabular-nums">{{ formatClockTime(entry.startedAtMs) }}</span>
            <span class="w-14 shrink-0 text-right text-muted-foreground tabular-nums">
              {{ entry.running ? "…" : formatDurationMs(entry.durationMs) }}
            </span>
            <span
              :class="
                cn(
                  'inline-flex h-4 min-w-6 shrink-0 items-center justify-center rounded border px-1 tabular-nums',
                  exitBadgeClass(entry),
                )
              "
              :title="entry.running ? 'Running' : `Exit code ${entry.exitCode ?? 'none'}`"
            >
              <LoaderCircleIcon v-if="entry.running" class="size-3 animate-spin" />
              <template v-else>{{ entry.exitCode ?? "—" }}</template>
            </span>
            <span
              :class="cn('h-4 shrink-0 rounded px-1.5 leading-4 font-sans text-[0.7rem]', categoryClass(entry.category))"
              :title="entry.label"
            >
              {{ categoryShort(entry.category) }}
            </span>
            <span class="truncate" :title="entry.display">{{ entry.display }}</span>
          </button>

          <div v-if="expanded.has(entry.seq)" class="space-y-1.5 bg-muted/20 px-7 py-1.5">
            <p class="font-sans text-[0.7rem] text-muted-foreground">
              {{ entry.label }}
              <template v-if="entry.stdoutBytes || entry.stderrBytes">
                · stdout {{ entry.stdoutBytes }} B · stderr {{ entry.stderrBytes }} B
              </template>
            </p>
            <p v-if="entry.error" class="font-sans text-xs text-destructive">
              {{ GitErrorMessage(entry.error) }}
            </p>
            <div v-if="entry.stdout.length > 0">
              <p class="font-sans text-[0.7rem] text-muted-foreground uppercase">
                stdout<span v-if="entry.stdoutTruncated" class="ml-1 text-modified-fg normal-case">(truncated)</span>
              </p>
              <pre class="max-h-64 overflow-auto rounded border border-border bg-background p-1.5 whitespace-pre scrollbar-thin">{{ entry.stdout }}</pre>
            </div>
            <div v-if="entry.stderr.length > 0">
              <p class="font-sans text-[0.7rem] text-muted-foreground uppercase">
                stderr<span v-if="entry.stderrTruncated" class="ml-1 text-modified-fg normal-case">(truncated)</span>
              </p>
              <pre class="max-h-64 overflow-auto rounded border border-border bg-background p-1.5 whitespace-pre text-modified-fg scrollbar-thin">{{ entry.stderr }}</pre>
            </div>
            <p
              v-if="!entry.running && entry.stdout.length === 0 && entry.stderr.length === 0 && !entry.error"
              class="font-sans text-[0.7rem] text-muted-foreground italic"
            >
              No output.
            </p>
          </div>
        </div>
      </div>
    </template>
  </section>
</template>
