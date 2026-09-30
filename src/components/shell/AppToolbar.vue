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

const props = defineProps<{
  /** False when no repository tab is open, which disables Commit. */
  hasRepo: boolean;
}>();

const emit = defineEmits<{
  addRepo: [];
  refresh: [];
  commit: [];
}>();

interface ToolbarAction {
  label: string;
  /** Short caption under the icon; a key is 4.5rem wide, so long labels are cut down. */
  caption: string;
  icon: Component;
  /** Set on buttons that are not built yet: they render disabled with this as the tooltip. */
  coming?: string;
  /** Emitted when a built button is clicked. */
  event?: "commit";
}

const M2 = "Coming in milestone 2";
const M3 = "Coming in milestone 3";

const groups: ToolbarAction[][] = [
  [
    { label: "Pull", caption: "Pull", icon: ArrowDownToLineIcon, coming: M3 },
    { label: "Sync", caption: "Sync", icon: ArrowDownUpIcon, coming: M3 },
    { label: "Push", caption: "Push", icon: ArrowUpFromLineIcon, coming: M3 },
    { label: "Commit", caption: "Commit", icon: GitCommitHorizontalIcon, event: "commit" },
  ],
  [
    { label: "Stage", caption: "Stage", icon: ListPlusIcon, coming: M2 },
    { label: "Unstage", caption: "Unstage", icon: ListMinusIcon, coming: M2 },
    { label: "Discard", caption: "Discard", icon: Undo2Icon, coming: M2 },
  ],
  [
    { label: "Save Stash", caption: "Save", icon: ArchiveIcon, coming: M2 },
    { label: "Apply Stash", caption: "Apply", icon: ArchiveRestoreIcon, coming: M2 },
  ],
  [
    { label: "Merge", caption: "Merge", icon: GitMergeIcon, coming: M3 },
    { label: "Rebase", caption: "Rebase", icon: RotateCcwIcon, coming: M3 },
  ],
];

function run(action: ToolbarAction): void {
  if (action.event === "commit") emit("commit");
}

/** Chiclet key: icon on top, caption below. The bottom margin leaves room for the 2px lip. */
const KEY_CLASS =
  "key relative mb-[2px] inline-flex h-[3rem] w-[4.5rem] shrink-0 flex-col items-center justify-between rounded-lg border px-0.5 pt-[5px] pb-1 text-[10px] leading-none font-medium whitespace-nowrap outline-none focus-visible:ring-3 focus-visible:ring-ring/50 disabled:opacity-45 [&_svg]:size-6 [&_svg]:shrink-0";
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

    <template v-for="(group, gi) in groups" :key="gi">
      <Separator orientation="vertical" class="mx-[5px] h-[32px]" />
      <template v-for="action in group" :key="action.label">
        <button
          v-if="!action.coming"
          type="button"
          :class="KEY_CLASS"
          :title="action.label"
          :disabled="!props.hasRepo"
          @click="run(action)"
        >
          <component :is="action.icon" />
          <span :class="CAPTION_CLASS">{{ action.caption }}</span>
        </button>
        <Tooltip v-else>
          <TooltipTrigger as-child>
            <!-- A disabled button swallows pointer events, so the wrapper span carries the tooltip. -->
            <span class="inline-flex">
              <button type="button" :class="KEY_CLASS" :aria-label="action.label" disabled>
                <component :is="action.icon" />
                <span :class="CAPTION_CLASS">{{ action.caption }}</span>
              </button>
            </span>
          </TooltipTrigger>
          <TooltipContent side="bottom">{{ action.coming }}</TooltipContent>
        </Tooltip>
      </template>
    </template>
  </header>
</template>
