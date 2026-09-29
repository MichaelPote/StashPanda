<script setup lang="ts">
import type { Component } from "vue";
import {
  ArchiveIcon,
  ArchiveRestoreIcon,
  ArrowDownToLineIcon,
  ArrowDownUpIcon,
  ArrowUpFromLineIcon,
  FolderOpenIcon,
  GitCommitHorizontalIcon,
  GitMergeIcon,
  ListMinusIcon,
  ListPlusIcon,
  RefreshCwIcon,
  RotateCcwIcon,
  Undo2Icon,
} from "@lucide/vue";
import { Separator } from "@/components/ui/separator";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";

const emit = defineEmits<{
  addRepo: [];
  refresh: [];
}>();

interface ToolbarAction {
  label: string;
  /** Short caption under the icon; a key is 4.5rem wide, so long labels are cut down. */
  caption: string;
  icon: Component;
}

/** Actions that ship in milestone 2. Rendered disabled with a tooltip. */
const comingGroups: ToolbarAction[][] = [
  [
    { label: "Pull", caption: "Pull", icon: ArrowDownToLineIcon },
    { label: "Sync", caption: "Sync", icon: ArrowDownUpIcon },
    { label: "Push", caption: "Push", icon: ArrowUpFromLineIcon },
    { label: "Commit", caption: "Commit", icon: GitCommitHorizontalIcon },
  ],
  [
    { label: "Stage", caption: "Stage", icon: ListPlusIcon },
    { label: "Unstage", caption: "Unstage", icon: ListMinusIcon },
    { label: "Discard", caption: "Discard", icon: Undo2Icon },
  ],
  [
    { label: "Save Stash", caption: "Save", icon: ArchiveIcon },
    { label: "Apply Stash", caption: "Apply", icon: ArchiveRestoreIcon },
  ],
  [
    { label: "Merge", caption: "Merge", icon: GitMergeIcon },
    { label: "Rebase", caption: "Rebase", icon: RotateCcwIcon },
  ],
];

/** Chiclet key: icon on top, caption below. The bottom margin leaves room for the 2px lip. */
const KEY_CLASS =
  "key relative mb-[2px] inline-flex h-[3rem] w-[4.5rem] shrink-0 flex-col items-center justify-between rounded-lg border px-0.5 pt-[5px] pb-1 text-[10px] leading-none font-medium whitespace-nowrap outline-none focus-visible:ring-3 focus-visible:ring-ring/50 disabled:opacity-45 [&_svg]:size-4 [&_svg]:shrink-0";
const CAPTION_CLASS = "block max-w-[59px] truncate text-[9px] font-medium tracking-[0.04em] uppercase";
</script>

<template>
  <header
    class="flex h-[52px] shrink-0 items-center gap-1 border-b border-border bg-bar px-1.5 select-none"
  >
    <button type="button" :class="KEY_CLASS" title="Open Repository" @click="emit('addRepo')">
      <FolderOpenIcon />
      <span :class="CAPTION_CLASS">Open</span>
    </button>
    <button type="button" :class="KEY_CLASS" title="Refresh" @click="emit('refresh')">
      <RefreshCwIcon />
      <span :class="CAPTION_CLASS">Refresh</span>
    </button>

    <template v-for="(group, gi) in comingGroups" :key="gi">
      <Separator orientation="vertical" class="mx-[5px] h-[32px]" />
      <Tooltip v-for="action in group" :key="action.label">
        <TooltipTrigger as-child>
          <!-- A disabled button swallows pointer events, so the wrapper span carries the tooltip. -->
          <span class="inline-flex">
            <button type="button" :class="KEY_CLASS" :aria-label="action.label" disabled>
              <component :is="action.icon" />
              <span :class="CAPTION_CLASS">{{ action.caption }}</span>
            </button>
          </span>
        </TooltipTrigger>
        <TooltipContent side="bottom">Coming in milestone 2</TooltipContent>
      </Tooltip>
    </template>
  </header>
</template>
