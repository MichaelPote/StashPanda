<script setup lang="ts">
import { computed } from "vue";
import { EyeIcon, GitBranchIcon, LoaderCircleIcon, MonitorIcon, ServerIcon } from "@lucide/vue";
import { Separator } from "@/components/ui/separator";
import { hostLabel, transportDetail } from "@/lib/format";
import { stopOperation } from "@/lib/repoActions";
import { useHostStore } from "@/stores/host";
import { useSessionsStore } from "@/stores/sessions";

const host = useHostStore();
const sessions = useSessionsStore();

const hostText = computed(() => hostLabel(host.info));
const active = computed(() => sessions.active);
/** The active repository's fetch, pull or push, if one is running. */
const operation = computed(() => active.value?.operation ?? null);

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
    class="flex h-[21px] shrink-0 items-center gap-2 border-t border-window bg-bar px-2 text-xs text-muted-foreground select-none"
  >
    <span v-if="hostText" class="inline-flex items-center gap-1" :title="'Host operating system'">
      <MonitorIcon class="size-3" />
      {{ hostText }}
    </span>
    <template v-if="active">
      <Separator orientation="vertical" class="h-3" />
      <span class="inline-flex items-center gap-1 truncate" :title="active.info.nativePath">
        <ServerIcon class="size-3" />
        {{ transportDetail(active.info.transport) }}
      </span>
      <Separator orientation="vertical" class="h-3" />
      <span class="inline-flex items-center gap-1">
        <GitBranchIcon class="size-3" />
        git {{ active.info.gitVersion }}
      </span>
      <template v-if="watcherText">
        <Separator orientation="vertical" class="h-3" />
        <span class="inline-flex items-center gap-1" title="How changes in this repository are detected">
          <EyeIcon class="size-3" />
          {{ watcherText }}
        </span>
      </template>
    </template>
    <span v-else class="ml-auto">No repository open</span>
    <!-- The running fetch, pull or push: what it is, Git's latest progress line, and Stop. -->
    <div v-if="active && operation" class="ml-auto flex min-w-0 items-center gap-1.5" role="status" aria-live="polite">
      <LoaderCircleIcon class="size-3 shrink-0 animate-spin" />
      <span class="shrink-0 text-foreground">{{ operation.title }}</span>
      <span v-if="operation.line || operation.stopping" class="min-w-0 truncate font-mono" :title="operation.line">
        {{ operation.stopping ? "Stopping…" : operation.line }}
      </span>
      <button
        type="button"
        class="shrink-0 rounded border border-border px-1.5 leading-[15px] text-foreground enabled:hover:bg-foreground/10 disabled:opacity-40"
        :disabled="!operation.cancellable || operation.stopping"
        :title="operation.cancellable ? 'Stop it. Git exits cleanly and nothing is half done.' : 'This step cannot be stopped.'"
        @click="stopOperation(active.key)"
      >
        Stop
      </button>
    </div>
  </footer>
</template>
