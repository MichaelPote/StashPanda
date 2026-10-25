<script setup lang="ts">
import { computed, nextTick, ref, watch } from "vue";
import { useResizeObserver } from "@vueuse/core";
import {
  CheckIcon,
  ChevronDownIcon,
  ChevronRightIcon,
  LoaderCircleIcon,
  PanelBottomOpenIcon,
  XIcon,
} from "@lucide/vue";
import type { Category } from "@/lib/bindings";
import ActionMenuItem from "@/components/graph/ActionMenuItem.vue";
import { ContextMenu, ContextMenuContent, ContextMenuTrigger } from "@/components/ui/context-menu";
import { Tip, Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { formatClockTime, formatDurationMs } from "@/lib/format";
import { GitErrorMessage } from "@/lib/ipc";
import { copyText } from "@/lib/repoActions";
import { cn } from "@/lib/utils";
import type { ConsoleEntry, RepoSession } from "@/stores/sessions";
import { vPixelSnap } from "@/lib/pixelSnap";
import { useSettingsStore } from "@/stores/settings";
import { useUiStateStore } from "@/stores/uiState";

const props = defineProps<{
  session: RepoSession | null;
  collapsed: boolean;
}>();

const uiState = useUiStateStore();

const settings = useSettingsStore();
/** `console.logInternal`: off, the app's own commands are not logged, so their button is hidden. */
const logInternal = computed(() => settings.settings?.console?.logInternal ?? false);

/** The filter buttons; `help` is each one's tooltip. */
const ALL_CATEGORIES: { id: Category; label: string; help: string }[] = [
  { id: "userAction", label: "User", help: "Commands from your own actions: commits, checkouts, fetches and the rest." },
  { id: "internal", label: "Internal", help: "Commands the app runs by itself: reading the repository's history, branches and status, and watching it for changes." },
];
const CATEGORIES = computed(() => ALL_CATEGORIES.filter((c) => c.id !== "internal" || logInternal.value));

type ConsoleButtons = Record<Category, boolean> & { log: boolean };

/** The Log, User and Internal buttons, remembered in the UI state. Log mode is the default. */
const filters = uiState.pref<ConsoleButtons>(
  (s) =>
    s.console
      ? {
          userAction: s.console.userAction ?? false,
          internal: s.console.internal ?? false,
          log: s.console.log ?? true,
        }
      : null,
  { userAction: false, internal: false, log: true },
  (value) => ({ console: value }),
);

/** Log mode: the standard output of the user's own commands, as plain text. */
const logMode = computed(() => filters.value.log);

/** In Log mode, a category button leaves Log mode with just that category on. */
function toggleFilter(id: Category): void {
  const f = filters.value;
  filters.value = { ...f, log: false, [id]: f.log ? true : !f[id] };
}

/** Log switches the others off; switching Log off turns them all back on. */
function toggleLog(): void {
  filters.value = filters.value.log ? { userAction: true, internal: true, log: false } : { userAction: false, internal: false, log: true };
}

/** Newest first. */
const entries = computed<ConsoleEntry[]>(() => {
  const all = props.session?.consoleEntries ?? [];
  const shown = logMode.value
    ? all.filter((e) => e.category === "userAction")
    : all.filter((e) => filters.value[e.category] !== false && (e.category !== "internal" || logInternal.value));
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

/**
 * Whether Log mode has anything to show for the entry. Standard error counts: push, fetch, pull,
 * clone and checkout write what they did there ("To github.com:…", "main -> main", rejections,
 * the server's own messages), and leave standard output empty.
 */
function hasLogOutput(entry: ConsoleEntry): boolean {
  return entry.stdout.length > 0 || entry.stderr.length > 0;
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

/** What the command printed, for Copy Output: standard output, then standard error. */
function outputText(entry: ConsoleEntry): string {
  return [entry.stdout, entry.stderr]
    .filter((t) => t.length > 0)
    .map((t) => t.replace(/\n+$/, ""))
    .join("\n");
}

/** The whole entry, for Copy: the command line as run, what it printed, and the error that stopped it. */
function entryText(entry: ConsoleEntry): string {
  const parts = [`$ ${entry.display}`, outputText(entry)];
  if (entry.error) parts.push(GitErrorMessage(entry.error));
  return parts.filter((p) => p.length > 0).join("\n");
}

/** Why Copy Output has nothing to copy, or null when it has. */
function noOutputReason(entry: ConsoleEntry): string | null {
  if (entry.stdout.length > 0 || entry.stderr.length > 0) return null;
  if (entry.running) return "The command has not printed anything yet.";
  if (entry.outputOmitted) return "Successful internal and watcher commands keep only the size of their output.";
  return "The command printed nothing.";
}

function categoryClass(category: Category): string {
  switch (category) {
    case "userAction":
      return "bg-violet/28 text-primary-tint";
    case "internal":
      return "bg-muted text-muted-foreground";
  }
}

function categoryShort(category: Category): string {
  switch (category) {
    case "userAction":
      return "user";
    case "internal":
      return "internal";
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
        <Tooltip>
          <TooltipTrigger as-child>
            <button
              type="button"
              :aria-pressed="logMode"
              class="toggle mr-1 h-[18px] rounded-sm border px-2 text-[0.7rem] leading-none font-medium transition-colors"
              @click="toggleLog"
            >
              Log
            </button>
          </TooltipTrigger>
          <TooltipContent side="top">The standard output of your own commands, as plain text</TooltipContent>
        </Tooltip>
        <Tip v-for="c in CATEGORIES" :key="c.id" :content="c.help" side="top">
          <button
            type="button"
            :aria-pressed="!logMode && filters[c.id] !== false"
            class="toggle h-[18px] rounded-sm border px-2 text-[0.7rem] leading-none font-medium transition-colors"
            @click="toggleFilter(c.id)"
          >
            {{ c.label }}
          </button>
        </Tip>
      </div>

      <span v-if="!collapsed && session" class="ml-auto text-[0.7rem] text-muted-foreground tabular-nums">
        {{ entries.length }} of {{ session.consoleEntries.length }}
      </span>

      <!-- 17px tall, so they sit 3px from the top and bottom of the 23px under the border; the
           12px close icon goes 2px from the top, on a whole pixel (see index.css). v-pixel-snap:
           against the pane's right edge, which can fall part-way through a pixel. -->
      <div v-pixel-snap :class="cn('flex items-center gap-[4px]', collapsed && 'ml-auto')">
        <button
          v-if="!collapsed && session"
          type="button"
          class="h-[17px] rounded-sm border border-border-soft px-2 text-[0.7rem] leading-none font-medium text-foreground transition-colors hover:bg-ghost-hover"
          @click="clear"
        >
          Clear
        </button>
        <Tooltip>
          <TooltipTrigger as-child>
            <button
              type="button"
              class="inline-flex h-[17px] w-[18px] items-start justify-center rounded-sm pt-[2px] text-muted-foreground transition-colors hover:bg-ghost-hover hover:text-foreground"
              :aria-label="collapsed ? 'Show the console' : 'Close the console'"
              @click="uiState.setConsoleCollapsed(!collapsed)"
            >
              <PanelBottomOpenIcon v-if="collapsed" class="size-3.5" />
              <XIcon v-else class="size-3.5" />
            </button>
          </TooltipTrigger>
          <TooltipContent side="top">{{ collapsed ? "Show the console" : "Close the console" }}</TooltipContent>
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
          <ContextMenu>
          <ContextMenuTrigger as-child>
          <button
            type="button"
            class="flex h-(--row-h) w-full items-center gap-2 px-2 text-left whitespace-pre hover:bg-row-hover"
            @click="toggleExpanded(entry)"
          >
            <component
              :is="isExpanded(entry) ? ChevronDownIcon : ChevronRightIcon"
              :class="cn('size-3 shrink-0 text-muted-foreground', logMode && !hasLogOutput(entry) && 'invisible')"
            />
            <span v-if="!logMode" class="shrink-0 text-muted-foreground tabular-nums">{{ formatClockTime(entry.startedAtMs) }}</span>
            <!-- How long it took: a tick and green for exit code 0, a cross and red with the code after a dash otherwise. -->
            <Tip :content="resultTitle(entry)" side="top">
              <span
                :class="
                  cn(
                    'inline-flex h-4 min-w-16 shrink-0 items-center justify-center gap-1 rounded border px-1.5 tabular-nums',
                    resultClass(entry),
                  )
                "
              >
                <LoaderCircleIcon v-if="entry.running" class="size-3 animate-spin" />
                <template v-else>
                  <CheckIcon v-if="succeeded(entry)" class="size-3 shrink-0" aria-label="Succeeded" />
                  <XIcon v-else class="size-3 shrink-0" aria-label="Failed" />
                  {{ resultText(entry) }}
                </template>
              </span>
            </Tip>
            <template v-if="logMode">
              <!-- What the command does, the git command alone, then the full command line off to the right. -->
              <Tip :content="entry.title" side="top">
                <span class="w-36 shrink-0 truncate font-sans font-medium text-foreground">{{ entry.title }}</span>
              </Tip>
              <Tip :content="entry.display" side="top">
                <span class="shrink-0 text-foreground">{{ entry.command }}</span>
              </Tip>
              <span class="shrink-0 pr-2 pl-6 text-muted-foreground/70">{{ entry.display }}</span>
            </template>
            <template v-else>
              <Tip :content="entry.label" side="top">
                <span :class="cn('h-4 shrink-0 rounded px-1.5 leading-4 font-sans text-[0.7rem]', categoryClass(entry.category))">
                  {{ categoryShort(entry.category) }}
                </span>
              </Tip>
              <Tip :content="entry.title" side="top">
                <span class="w-36 shrink-0 truncate font-sans font-medium text-foreground">{{ entry.title }}</span>
              </Tip>
              <span class="shrink-0 pr-2">{{ entry.display }}</span>
            </template>
          </button>
          </ContextMenuTrigger>
          <ContextMenuContent class="min-w-44">
            <ActionMenuItem @select="copyText(entryText(entry), 'the command')">Copy</ActionMenuItem>
            <ActionMenuItem :reason="noOutputReason(entry)" @select="copyText(outputText(entry), 'the output')">Copy Output</ActionMenuItem>
          </ContextMenuContent>
          </ContextMenu>

          <!-- Log mode: what the command printed, as plain text, pinned to the visible part of the
               list. Standard output, then standard error, which is where Git reports on a push,
               fetch, pull or checkout; red only when the command failed. -->
          <div
            v-if="logMode && isExpanded(entry) && hasLogOutput(entry)"
            class="sticky left-0 overflow-x-auto px-7 pb-1.5 select-text scrollbar-thin"
            :style="viewWidth > 0 ? { width: `${viewWidth}px` } : undefined"
          >
            <pre v-if="entry.stdout.length > 0" class="whitespace-pre text-text-2">{{ entry.stdout }}</pre>
            <pre v-if="entry.stderr.length > 0" :class="cn('whitespace-pre', succeeded(entry) || entry.running ? 'text-text-2' : 'text-destructive')">{{ entry.stderr }}</pre>
            <p v-if="entry.stdoutTruncated || entry.stderrTruncated" class="font-sans text-[0.7rem] text-modified-fg">The output was cut off at the console's size limit.</p>
          </div>

          <!-- Pinned to the visible part of the list, with its own scrollbars for long output. -->
          <div
            v-else-if="!logMode && isExpanded(entry)"
            class="sticky left-0 space-y-1.5 bg-muted/20 px-7 py-1.5 select-text"
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
