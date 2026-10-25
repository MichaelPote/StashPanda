<script setup lang="ts">
import { FolderRootIcon, XIcon } from "@lucide/vue";
import { vPixelSnap } from "@/lib/pixelSnap";
import { cn } from "@/lib/utils";
import { useLibraryStore } from "@/stores/library";
import { useSessionsStore } from "@/stores/sessions";

const sessions = useSessionsStore();
const library = useLibraryStore();

function onAuxClick(event: MouseEvent, key: string): void {
  // Button 1 is the middle button; closing on it matches browser and IDE tab strips.
  if (event.button === 1) {
    event.preventDefault();
    void sessions.closeTab(key);
  }
}

/**
 * Keyboard use, as in any tab list: Tab reaches only the active tab; the Left and Right arrows,
 * Home and End move between tabs; Enter or Space switches to the focused one, and Delete closes it.
 * Moving does not switch by itself, since switching loads a repository.
 */
function onKeydown(event: KeyboardEvent, key: string): void {
  const tabs = [...((event.currentTarget as HTMLElement).parentElement?.querySelectorAll<HTMLElement>('[role="tab"]') ?? [])];
  const at = tabs.indexOf(event.currentTarget as HTMLElement);
  const moveTo = (index: number): void => {
    event.preventDefault();
    tabs[(index + tabs.length) % tabs.length]?.focus();
  };
  if (event.key === "ArrowRight") moveTo(at + 1);
  else if (event.key === "ArrowLeft") moveTo(at - 1);
  else if (event.key === "Home") moveTo(0);
  else if (event.key === "End") moveTo(tabs.length - 1);
  else if (event.key === "Enter" || event.key === " ") {
    event.preventDefault();
    sessions.setActive(key);
  } else if (event.key === "Delete") {
    event.preventDefault();
    void sessions.closeTab(key);
  }
}
</script>

<template>
  <div
    class="flex h-[28px] shrink-0 items-stretch gap-0.5 overflow-x-auto border-b border-border bg-bar px-1 select-none scrollbar-thin"
    role="tablist"
  >
    <div
      v-for="session in sessions.list"
      :key="session.key"
      role="tab"
      :tabindex="session.key === sessions.activeKey ? 0 : -1"
      :aria-selected="session.key === sessions.activeKey"
      :class="
        cn(
          // Every tab has the same box (3px top margin, 24px tall, a 1px border), so becoming active
          // changes only colours and never moves the label.
          'group relative mt-[3px] flex h-[24px] min-w-0 max-w-56 shrink-0 cursor-default items-center gap-1.5 rounded-t-lg border border-b-0 px-[11px] text-[12.5px] transition-colors',
          session.key === sessions.activeKey
            ? 'border-primary/60 bg-primary/20 text-foreground shadow-[inset_0_2px_0_var(--primary)]'
            : 'border-transparent text-muted-foreground hover:bg-ghost-hover hover:text-foreground',
        )
      "
      @click="sessions.setActive(session.key)"
      @auxclick="onAuxClick($event, session.key)"
      @mousedown.middle.prevent
      @keydown="onKeydown($event, session.key)"
    >
      <!-- The active tab's icon is the accent, which is its repository's colour when it has one.
           Snapped to whole pixels: each tab starts wherever the text of the one before it ends. -->
      <span v-pixel-snap class="inline-flex shrink-0">
        <FolderRootIcon
          :class="cn('size-3.5 shrink-0', session.key === sessions.activeKey && 'text-primary')"
          :style="session.key !== sessions.activeKey && library.repoByKey.get(session.key)?.colour != null ? { color: `var(--graph-lane-${library.repoByKey.get(session.key)?.colour})` } : undefined"
        />
      </span>
      <span class="truncate">{{ session.info.displayName }}</span>
      <button
        type="button"
        :class="
          cn(
            'ml-1 inline-flex size-4 shrink-0 items-center justify-center rounded-sm hover:bg-ghost-hover',
            session.key === sessions.activeKey ? 'opacity-100' : 'opacity-0 group-hover:opacity-100 group-focus-visible:opacity-100',
          )
        "
        tabindex="-1"
        :aria-label="`Close ${session.info.displayName}`"
        @click.stop="sessions.closeTab(session.key)"
      >
        <XIcon class="size-3" />
      </button>
    </div>
  </div>
</template>
