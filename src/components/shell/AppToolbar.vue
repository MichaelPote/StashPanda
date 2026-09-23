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
import { Button } from "@/components/ui/button";
import { Separator } from "@/components/ui/separator";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";

const emit = defineEmits<{
  addRepo: [];
  refresh: [];
}>();

interface ToolbarAction {
  label: string;
  icon: Component;
}

/** Actions that ship in milestone 2. Rendered disabled with a tooltip. */
const comingGroups: ToolbarAction[][] = [
  [
    { label: "Pull", icon: ArrowDownToLineIcon },
    { label: "Sync", icon: ArrowDownUpIcon },
    { label: "Push", icon: ArrowUpFromLineIcon },
    { label: "Commit", icon: GitCommitHorizontalIcon },
  ],
  [
    { label: "Stage", icon: ListPlusIcon },
    { label: "Unstage", icon: ListMinusIcon },
    { label: "Discard", icon: Undo2Icon },
  ],
  [
    { label: "Save Stash", icon: ArchiveIcon },
    { label: "Apply Stash", icon: ArchiveRestoreIcon },
  ],
  [
    { label: "Merge", icon: GitMergeIcon },
    { label: "Rebase", icon: RotateCcwIcon },
  ],
];
</script>

<template>
  <header
    class="flex h-9 shrink-0 items-center gap-0.5 border-b border-border chrome px-1.5 select-none"
  >
    <Button variant="ghost" size="sm" class="h-7 px-2 text-base" @click="emit('addRepo')">
      <FolderOpenIcon />
      Open Repository
    </Button>
    <Button variant="ghost" size="sm" class="h-7 px-2 text-base" @click="emit('refresh')">
      <RefreshCwIcon />
      Refresh
    </Button>

    <template v-for="(group, gi) in comingGroups" :key="gi">
      <Separator orientation="vertical" class="mx-1 h-5" />
      <Tooltip v-for="action in group" :key="action.label">
        <TooltipTrigger as-child>
          <!-- A disabled button swallows pointer events, so the wrapper span carries the tooltip. -->
          <span class="inline-flex">
            <Button variant="ghost" size="sm" class="h-7 px-2 text-base" disabled>
              <component :is="action.icon" />
              {{ action.label }}
            </Button>
          </span>
        </TooltipTrigger>
        <TooltipContent side="bottom">Coming in milestone 2</TooltipContent>
      </Tooltip>
    </template>
  </header>
</template>
