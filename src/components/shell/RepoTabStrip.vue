<script setup lang="ts">
import { FolderGit2Icon, XIcon } from "@lucide/vue";
import { transportLabel } from "@/lib/format";
import { cn } from "@/lib/utils";
import { useSessionsStore } from "@/stores/sessions";

const sessions = useSessionsStore();

function onAuxClick(event: MouseEvent, key: string): void {
  // Button 1 is the middle button; closing on it matches browser and IDE tab strips.
  if (event.button === 1) {
    event.preventDefault();
    void sessions.closeTab(key);
  }
}
</script>

<template>
  <div
    class="flex h-8 shrink-0 items-stretch overflow-x-auto border-b border-border chrome select-none scrollbar-thin"
    role="tablist"
  >
    <div
      v-for="session in sessions.list"
      :key="session.key"
      role="tab"
      tabindex="0"
      :aria-selected="session.key === sessions.activeKey"
      :class="
        cn(
          'group relative flex min-w-0 max-w-56 shrink-0 cursor-default items-center gap-1.5 border-r border-border px-2.5 text-base',
          session.key === sessions.activeKey
            ? 'bg-background text-foreground after:absolute after:inset-x-0 after:top-0 after:h-px after:bg-primary'
            : 'text-muted-foreground hover:bg-accent/50 hover:text-foreground',
        )
      "
      :title="`${session.info.nativePath} (${transportLabel(session.info.transport)})`"
      @click="sessions.setActive(session.key)"
      @auxclick="onAuxClick($event, session.key)"
      @mousedown.middle.prevent
      @keydown.enter="sessions.setActive(session.key)"
    >
      <FolderGit2Icon class="size-3.5 shrink-0" />
      <span class="truncate">{{ session.info.displayName }}</span>
      <button
        type="button"
        :class="
          cn(
            'ml-1 inline-flex size-4 shrink-0 items-center justify-center rounded-sm hover:bg-ghost-hover',
            session.key === sessions.activeKey ? 'opacity-100' : 'opacity-0 group-hover:opacity-100',
          )
        "
        :aria-label="`Close ${session.info.displayName}`"
        @click.stop="sessions.closeTab(session.key)"
      >
        <XIcon class="size-3" />
      </button>
    </div>
  </div>
</template>
