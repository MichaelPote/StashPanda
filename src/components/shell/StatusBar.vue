<script setup lang="ts">
import { Tip } from "@/components/ui/tooltip";
import { computed, onBeforeUnmount, ref, watch } from "vue";
import { CircleCheckIcon, EyeIcon, GitBranchIcon, InfoIcon, LoaderCircleIcon, MonitorIcon, ServerIcon, UnplugIcon, XIcon } from "@lucide/vue";
import { Separator } from "@/components/ui/separator";
import { hostLabel, transportDetail } from "@/lib/format";
import { cn } from "@/lib/utils";
import { stopOperation } from "@/lib/repoActions";
import { useHostStore } from "@/stores/host";
import { useSessionsStore } from "@/stores/sessions";

const host = useHostStore();
const sessions = useSessionsStore();

const hostText = computed(() => hostLabel(host.info));
const active = computed(() => sessions.active);
/** The active repository's fetch, pull or push, if one is running. */
const operation = computed(() => active.value?.operation ?? null);

/** True while the active repository's commit graph is being built, on first load and on every refresh. */
const graphLoading = computed(() => {
  const graph = active.value?.graph;
  if (!graph) return false;
  // Only the loading flag: a load that failed leaves no snapshot (version 0) and is not loading.
  return graph.loading;
});
/** Commits loaded so far: the backend's progress count, or the previous snapshot's total if that is larger. */
const loadedCount = computed(() => {
  const graph = active.value?.graph;
  return graph ? Math.max(graph.progress, graph.meta?.total ?? 0) : 0;
});

/** The last report that an action worked ("Committed …", "Stashed …"). Errors show in the strip under the toolbar. */
const successMessage = computed(() => sessions.successMessage);

// ---- The SSH connection's state ----------------------------------------------------------------

/** Ticks once a second while a countdown to the next connection attempt is shown. */
const now = ref(Date.now());
let ticker: ReturnType<typeof setInterval> | null = null;
watch(
  () => active.value?.connection?.state === "degraded",
  (counting) => {
    if (counting && ticker === null) {
      // Start from the current time: the clock stood still since the last countdown.
      now.value = Date.now();
      ticker = setInterval(() => (now.value = Date.now()), 1000);
    }
    if (!counting && ticker !== null) {
      clearInterval(ticker);
      ticker = null;
    }
  },
  { immediate: true },
);
onBeforeUnmount(() => {
  if (ticker !== null) clearInterval(ticker);
});

const connectionText = computed(() => {
  const c = active.value?.connection;
  switch (c?.state) {
    case "connecting":
      return "connecting…";
    case "degraded": {
      const secs = c.retryAt === null ? 0 : Math.ceil((c.retryAt - now.value) / 1000);
      return secs > 0 ? `connection lost, trying again in ${secs} s` : "connection lost, trying again";
    }
    case "disconnected":
      return "disconnected";
    default:
      return null;
  }
});
const connectionClass = computed(() => (active.value?.connection?.state === "disconnected" ? "text-destructive" : "text-modified-fg"));

const watcherText = computed(() => {
  switch (active.value?.watcherKind) {
    case "native":
      return "watching: native";
    case "remoteInotify":
      return "watching: inotify (remote)";
    case "polling":
      return "polling";
    case "off":
      return "watching: off";
    default:
      return null;
  }
});
</script>

<template>
  <footer
    class="flex h-[21px] shrink-0 items-center gap-2 border-t border-window bg-bar px-2 text-xs whitespace-nowrap text-muted-foreground select-none"
  >
    <span v-if="hostText" class="inline-flex shrink-0 items-center gap-1">
      <MonitorIcon class="size-3" />
      {{ hostText }}
    </span>
    <template v-if="active">
      <Separator orientation="vertical" class="h-3" />
      <Tip :content="active.info.nativePath" side="top">
        <span class="inline-flex min-w-0 items-center gap-1 truncate">
          <ServerIcon class="size-3 shrink-0" />
          {{ transportDetail(active.info.transport) }}
        </span>
      </Tip>
      <!-- An SSH connection that is not ready: connecting, lost and trying again, or refused. -->
      <Tip v-if="connectionText" :content="active.connection?.detail" side="top">
        <span :class="cn('inline-flex shrink-0 items-center gap-1', connectionClass)" role="status">
          <LoaderCircleIcon v-if="active.connection?.state === 'connecting'" class="size-3 animate-spin" />
          <UnplugIcon v-else class="size-3" />
          {{ connectionText }}
        </span>
      </Tip>
      <Separator orientation="vertical" class="h-3" />
      <span class="inline-flex shrink-0 items-center gap-1">
        <GitBranchIcon class="size-3" />
        git {{ active.info.gitVersion }}
      </span>
      <template v-if="watcherText">
        <Separator orientation="vertical" class="h-3" />
        <Tip :content="active?.watcherDetail ?? 'How changes in this repository are detected'" side="top">
          <span class="inline-flex shrink-0 items-center gap-1">
            <EyeIcon class="size-3" />
            {{ watcherText }}
            <!-- The watcher has something to explain, such as why it polls; the tooltip says what. -->
            <InfoIcon v-if="active?.watcherDetail" class="size-3 text-modified-fg" aria-label="See the tooltip for why" />
          </span>
        </Tip>
      </template>
    </template>
    <!-- The right-hand side: the last success message, the commit graph loading, and the running fetch, pull or push. -->
    <div class="ml-auto flex min-w-0 items-center gap-2">
      <div v-if="successMessage" class="flex min-w-0 items-center gap-1" role="status" aria-live="polite">
        <CircleCheckIcon class="size-3 shrink-0 text-added-fg" />
        <Tip :content="successMessage" side="top">
          <span class="min-w-0 truncate text-foreground">{{ successMessage }}</span>
        </Tip>
        <button type="button" class="shrink-0 rounded p-px hover:bg-foreground/10" aria-label="Dismiss" @click="sessions.dismissSuccess()">
          <XIcon class="size-3" />
        </button>
      </div>
      <template v-if="graphLoading">
        <Separator v-if="successMessage" orientation="vertical" class="h-3" />
        <span class="inline-flex shrink-0 items-center gap-1 tabular-nums" role="status">
          <LoaderCircleIcon class="size-3 shrink-0 animate-spin" />
          Loading {{ loadedCount.toLocaleString() }} commits…
        </span>
      </template>
      <!-- The running fetch, pull or push: what it is, Git's latest progress line, and Stop. -->
      <template v-if="active && operation">
        <Separator v-if="successMessage || graphLoading" orientation="vertical" class="h-3" />
        <div class="flex min-w-0 items-center gap-1.5" role="status" aria-live="polite">
          <LoaderCircleIcon class="size-3 shrink-0 animate-spin" />
          <span class="shrink-0 text-foreground">{{ operation.title }}</span>
          <Tip v-if="operation.line || operation.stopping" :content="operation.line" side="top">
            <span class="min-w-0 truncate font-mono">
              {{ operation.stopping ? "Stopping…" : operation.line }}
            </span>
          </Tip>
          <!-- A disabled button lets no pointer events through, so the span carries the tooltip. -->
          <Tip :content="operation.cancellable ? 'Stop it. Git exits cleanly and nothing is half done.' : 'This step cannot be stopped.'" side="top">
            <span class="inline-flex shrink-0">
              <button
                type="button"
                class="shrink-0 rounded border border-border px-1.5 leading-[15px] text-foreground enabled:hover:bg-foreground/10 disabled:opacity-40"
                :disabled="!operation.cancellable || operation.stopping"
                @click="stopOperation(active.key)"
              >
                Stop
              </button>
            </span>
          </Tip>
        </div>
      </template>
      <span v-if="!active" class="shrink-0">No repository open</span>
    </div>
  </footer>
</template>
