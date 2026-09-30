<script setup lang="ts">
import { computed, nextTick, ref, watch } from "vue";
import { useResizeObserver } from "@vueuse/core";
import {
  CheckIcon,
  ChevronDownIcon,
  ChevronRightIcon,
  LoaderCircleIcon,
  PanelBottomCloseIcon,
  PanelBottomOpenIcon,
  Trash2Icon,
  XIcon,
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

type ConsoleButtons = Record<Category, boolean> & { log: boolean };

/** The Log, User, Internal and Watcher buttons, remembered in the UI state. Log mode is the default. */
const filters = uiState.pref<ConsoleButtons>(
  (s) =>
    s.console
      ? {
          userAction: s.console.userAction ?? false,
          internal: s.console.internal ?? false,
          watcher: s.console.watcher ?? false,
          log: s.console.log ?? true,
        }
      : null,
  { userAction: false, internal: false, watcher: false, log: true },
  (value) => ({ console: value }),
);

/** Log mode: the standard output of the user's own commands, as plain text. */
const logMode = computed(() => filters.value.log);

/** In Log mode, a category button leaves Log mode with just that category on. */
function toggleFilter(id: Category): void {
  const f = filters.value;
  filters.value = { ...f, log: false, [id]: f.log ? true : !f[id] };
}

/** Log switches the other three off; switching Log off turns all three back on. */
function toggleLog(): void {
  filters.value = filters.value.log
    ? { userAction: true, internal: true, watcher: true, log: false }
    : { userAction: false, internal: false, watcher: false, log: true };
}

/** Newest first. */
const entries = computed<ConsoleEntry[]>(() => {
  const all = props.session?.consoleEntries ?? [];
  const shown = logMode.value
    ? all.filter((e) => e.category === "userAction")
    : all.filter((e) => filters.value[e.category] !== false);
  return shown.slice().reverse();
});

/** Rows opened in the normal view; Log mode instead starts every row open and tracks the closed ones. */
const expanded = ref(new Set<number>());
const collapsedInLog = ref(new Set<number>());

function isExpanded(entry: ConsoleEntry): boolean {
  return logMode.value ? !collapsedInLog.value.has(entry.seq) : expanded.value.has(entry.seq);
}

function toggleExpanded(entry: ConsoleEntry): void {
  const target = logMode.value ? collapsedInLog : expanded;
  const next = new Set(target.value);
  if (next.has(entry.seq)) next.delete(entry.seq);
  else next.add(entry.seq);
  target.value = next;
}

function clear(): void {
  props.session?.consoleEntries.splice(0);
  expanded.value = new Set();
  collapsedInLog.value = new Set();
}

const scroller = ref<HTMLDivElement | null>(null);
/** Visible width of the list, so expanded output stays panel-wide while the rows scroll sideways. */
const viewWidth = ref(0);
useResizeObserver(scroller, () => {
  viewWidth.value = scroller.value?.clientWidth ?? 0;
});

// New commands arrive at the top. At the top the list stays there; scrolled down, the browser's
// scroll anchoring keeps the rows being read in place.
watch(
  () => props.session?.key,
  async () => {
    expanded.value = new Set();
    collapsedInLog.value = new Set();
    await nextTick();
    if (scroller.value) scroller.value.scrollTop = 0;
  },
);

/** Green when the command exited 0, red otherwise; running commands are neutral. */
function resultClass(entry: ConsoleEntry): string {
  if (entry.running) return "border-border text-muted-foreground";
  if (succeeded(entry)) return "border-added/40 bg-added/15 text-added-fg";
  return "border-destructive/40 bg-destructive/15 text-destructive";
}

function succeeded(entry: ConsoleEntry): boolean {
  return entry.exitCode === 0 && !entry.error;
}

/** How long the command took, with the exit code after a dash when it failed. */
function resultText(entry: ConsoleEntry): string {
  const time = formatDurationMs(entry.durationMs);
  if (succeeded(entry)) return time;
  return `${time} - ${entry.exitCode ?? "failed"}`;
}

function resultTitle(entry: ConsoleEntry): string {
  if (entry.running) return "Running";
  return `Took ${formatDurationMs(entry.durationMs)}; exit code ${entry.exitCode ?? "none"}`;
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
          type="button"
          :aria-pressed="logMode"
          class="toggle mr-1 h-[18px] rounded-sm border px-2 text-[0.7rem] leading-none font-medium transition-colors"
          title="The standard output of your own commands, as plain text"
          @click="toggleLog"
        >
          Log
        </button>
        <button
          v-for="c in CATEGORIES"
          :key="c.id"
          type="button"
          :aria-pressed="!logMode && filters[c.id] !== false"
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
        {{
          session.consoleEntries.length === 0
            ? "No Git commands have run yet."
            : logMode
              ? "None of your own commands have run yet."
              : "No log messages"
        }}
      </div>
      <div v-else ref="scroller" class="min-h-0 flex-1 overflow-auto font-mono text-xs scrollbar-thin">
        <!-- Rows are as wide as the longest command line, so the list scrolls sideways instead of cutting it off. -->
        <div class="w-max min-w-full">
        <div v-for="entry in entries" :key="entry.seq" class="border-b border-border/60">
          <button
            type="button"
            class="flex h-(--row-h) w-full items-center gap-2 px-2 text-left whitespace-pre hover:bg-row-hover"
            @click="toggleExpanded(entry)"
          >
            <component
              :is="isExpanded(entry) ? ChevronDownIcon : ChevronRightIcon"
              :class="cn('size-3 shrink-0 text-muted-foreground', logMode && entry.stdout.length === 0 && 'invisible')"
            />
            <span v-if="!logMode" class="shrink-0 text-muted-foreground tabular-nums">{{ formatClockTime(entry.startedAtMs) }}</span>
            <!-- How long it took: a tick and green for exit code 0, a cross and red with the code after a dash otherwise. -->
            <span
              :class="
                cn(
                  'inline-flex h-4 min-w-16 shrink-0 items-center justify-center gap-1 rounded border px-1.5 tabular-nums',
                  resultClass(entry),
                )
              "
              :title="resultTitle(entry)"
            >
              <LoaderCircleIcon v-if="entry.running" class="size-3 animate-spin" />
              <template v-else>
                <CheckIcon v-if="succeeded(entry)" class="size-3 shrink-0" aria-label="Succeeded" />
                <XIcon v-else class="size-3 shrink-0" aria-label="Failed" />
                {{ resultText(entry) }}
              </template>
            </span>
            <template v-if="logMode">
              <!-- The git command alone, then the full command line off to the right. -->
              <span class="shrink-0 text-foreground" :title="entry.display">{{ entry.command }}</span>
              <span class="shrink-0 pr-2 pl-6 text-muted-foreground/70" :title="entry.display">{{ entry.display }}</span>
            </template>
            <template v-else>
              <span
                :class="cn('h-4 shrink-0 rounded px-1.5 leading-4 font-sans text-[0.7rem]', categoryClass(entry.category))"
                :title="entry.label"
              >
                {{ categoryShort(entry.category) }}
              </span>
              <span class="shrink-0 pr-2" :title="entry.display">{{ entry.display }}</span>
            </template>
          </button>

          <!-- Log mode: just the standard output, as plain text, pinned to the visible part of the list. -->
          <div
            v-if="logMode && isExpanded(entry) && entry.stdout.length > 0"
            class="sticky left-0 overflow-x-auto px-7 pb-1.5 scrollbar-thin"
            :style="viewWidth > 0 ? { width: `${viewWidth}px` } : undefined"
          >
            <pre class="whitespace-pre text-text-2">{{ entry.stdout }}</pre>
            <p v-if="entry.stdoutTruncated" class="font-sans text-[0.7rem] text-modified-fg">The output was cut off at the console's size limit.</p>
          </div>

          <!-- Pinned to the visible part of the list, with its own scrollbars for long output. -->
          <div
            v-else-if="!logMode && isExpanded(entry)"
            class="sticky left-0 space-y-1.5 bg-muted/20 px-7 py-1.5"
            :style="viewWidth > 0 ? { width: `${viewWidth}px` } : undefined"
          >
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
              <pre class="max-h-64 overflow-auto rounded border border-border bg-background p-1.5 whitespace-pre text-text-2 scrollbar-thin">{{ entry.stdout }}</pre>
            </div>
            <div v-if="entry.stderr.length > 0">
              <p class="font-sans text-[0.7rem] text-muted-foreground uppercase">
                stderr<span v-if="entry.stderrTruncated" class="ml-1 text-modified-fg normal-case">(truncated)</span>
              </p>
              <pre class="max-h-64 overflow-auto rounded border border-border bg-background p-1.5 whitespace-pre text-modified-fg scrollbar-thin">{{ entry.stderr }}</pre>
            </div>
            <p v-if="entry.outputOmitted" class="font-sans text-[0.7rem] text-muted-foreground italic">
              Output not kept: successful internal and watcher commands keep only the size of their output.
            </p>
            <p
              v-else-if="!entry.running && entry.stdout.length === 0 && entry.stderr.length === 0 && !entry.error"
              class="font-sans text-[0.7rem] text-muted-foreground italic"
            >
              No output.
            </p>
          </div>
        </div>
        </div>
      </div>
    </template>
  </section>
</template>
