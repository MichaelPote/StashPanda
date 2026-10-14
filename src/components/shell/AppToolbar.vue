<script setup lang="ts">
import type { Component } from "vue";
import {
  ArchiveIcon,
  ArrowDownToLineIcon,
  ArrowDownUpIcon,
  ArrowUpFromLineIcon,
  CloudDownloadIcon,
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
import { cn } from "@/lib/utils";

const props = defineProps<{
  /** False when no repository tab is open, which disables Commit. */
  hasRepo: boolean;
  /** Files the Commit dialog would list, badged on the Commit button; hidden at 0. */
  commitCount: number;
  /** Why a button cannot act right now; a button with a reason is disabled and shows it as its tooltip. */
  reasons?: Partial<Record<ToolbarEvent, string | null>>;
}>();

const emit = defineEmits<{
  addRepo: [];
  refresh: [];
  commit: [];
  stashSave: [];
  pull: [];
  fetch: [];
  push: [];
  sync: [];
  stage: [];
  unstage: [];
  discard: [];
}>();

type ToolbarEvent = "commit" | "stashSave" | "pull" | "fetch" | "push" | "sync" | "stage" | "unstage" | "discard";

interface ToolbarAction {
  label: string;
  /** Short caption under the icon; a button is 63px wide, so long labels are cut down. */
  caption: string;
  icon: Component;
  /** Set on buttons that are not built yet: they render disabled with this as the tooltip. */
  coming?: string;
  /** Emitted when a built button is clicked. */
  event?: ToolbarEvent;
  /** The main action: a solid primary button among the flat ones. */
  primary?: boolean;
}

const M3 = "Coming in milestone 3";

const groups: ToolbarAction[][] = [
  [
    { label: "Fetch from Every Remote", caption: "Fetch", icon: CloudDownloadIcon, event: "fetch" },
    { label: "Pull", caption: "Pull", icon: ArrowDownToLineIcon, event: "pull" },
    { label: "Sync: Pull, then Push", caption: "Sync", icon: ArrowDownUpIcon, event: "sync" },
    { label: "Push to Upstream", caption: "Push", icon: ArrowUpFromLineIcon, event: "push" },
    { label: "Commit", caption: "Commit", icon: GitCommitHorizontalIcon, event: "commit", primary: true },
  ],
  [
    { label: "Stage the Selected Files", caption: "Stage", icon: ListPlusIcon, event: "stage" },
    { label: "Unstage the Selected Files", caption: "Unstage", icon: ListMinusIcon, event: "unstage" },
    { label: "Discard Changes to the Selected Files…", caption: "Discard", icon: Undo2Icon, event: "discard" },
  ],
  [
    // No Apply Stash key: applying means choosing which stash, which the menus and the
    // Branches panel do. Saving always makes a new stash, so it can be one click.
    { label: "Stash Changes", caption: "Stash", icon: ArchiveIcon, event: "stashSave" },
  ],
  [
    { label: "Merge", caption: "Merge", icon: GitMergeIcon, coming: M3 },
    { label: "Rebase", caption: "Rebase", icon: RotateCcwIcon, coming: M3 },
  ],
];

function run(action: ToolbarAction): void {
  if (action.event === "commit") emit("commit");
  else if (action.event === "stashSave") emit("stashSave");
  else if (action.event === "pull") emit("pull");
  else if (action.event === "fetch") emit("fetch");
  else if (action.event === "push") emit("push");
  else if (action.event === "sync") emit("sync");
  else if (action.event === "stage") emit("stage");
  else if (action.event === "unstage") emit("unstage");
  else if (action.event === "discard") emit("discard");
}

/** Why the button cannot be used: not built yet, or the reason its event gives. Null when it can. */
function blocked(action: ToolbarAction): string | null {
  if (action.coming) return action.coming;
  if (!props.hasRepo || !action.event) return null;
  return props.reasons?.[action.event] ?? null;
}

/**
 * Flat toolbar button: icon on top, caption below, no fill or border at rest. Hover and press
 * lay the ink over the bar at 6% and 10%; disabled drops to 38% opacity. The 20px icons draw a
 * 1.67px line (Lucide's 2 units at 20px) where the app's other icons draw 1px.
 */
const BUTTON_CLASS =
  "relative inline-flex h-[42px] w-[63px] shrink-0 flex-col items-center justify-between rounded-[4px] px-[2px] pt-[6px] pb-[5px] text-[10px] leading-none font-medium whitespace-nowrap text-foreground/90 transition-colors duration-[120ms] outline-none enabled:hover:bg-foreground/6 enabled:hover:text-foreground enabled:active:bg-foreground/10 focus-visible:ring-3 focus-visible:ring-ring/50 disabled:opacity-38 [&_svg]:size-5 [&_svg]:shrink-0 [&_svg]:[stroke-width:1.67px]";
/** The primary button: solid primary fill, a step towards the primary colour on hover, darker pressed. */
const PRIMARY_CLASS =
  "bg-primary-fill text-primary-fill-foreground enabled:hover:bg-[color-mix(in_srgb,var(--primary-fill)_40%,var(--primary))] enabled:hover:text-primary-fill-foreground enabled:active:bg-[color-mix(in_srgb,var(--primary-fill)_85%,black)]";

function buttonClass(action?: ToolbarAction): string {
  return cn(BUTTON_CLASS, action?.primary && PRIMARY_CLASS);
}

/** Count badge in a button's top right corner, in the primary fill. */
const BADGE_CLASS =
  "absolute top-[3px] right-[3px] inline-flex h-[15px] items-center rounded-[3px] bg-primary-fill px-[3px] text-[9px] leading-none text-primary-fill-foreground tabular-nums";

/** The Commit button's badge text: the file count, capped at 999+; null for none or other buttons. */
function badge(action: ToolbarAction): string | null {
  if (action.event !== "commit" || props.commitCount <= 0) return null;
  return props.commitCount > 999 ? "999+" : String(props.commitCount);
}
const CAPTION_CLASS = "block max-w-[59px] truncate text-[9px] font-medium tracking-[0.04em] uppercase";
</script>

<template>
  <header
    class="flex h-[52px] shrink-0 items-center border-b border-border bg-bar px-[6px] select-none"
  >
    <button type="button" :class="buttonClass()" title="Open Repository" @click="emit('addRepo')">
      <FolderOpenIcon />
      <span :class="CAPTION_CLASS">Open</span>
    </button>
    <button type="button" :class="buttonClass()" title="Refresh" @click="emit('refresh')">
      <RefreshCwIcon />
      <span :class="CAPTION_CLASS">Refresh</span>
    </button>

    <template v-for="(group, gi) in groups" :key="gi">
      <!-- data-[orientation=vertical]:self-center replaces the Separator's own self-stretch, which
           would pin a fixed-height divider to the top of the bar. -->
      <Separator orientation="vertical" class="mx-[5px] h-[32px] bg-(--key) data-[orientation=vertical]:self-center" />
      <template v-for="action in group" :key="action.label">
        <button
          v-if="!blocked(action)"
          type="button"
          :class="buttonClass(action)"
          :title="badge(action) ? `${action.label} (${badge(action)} ${props.commitCount === 1 ? 'file' : 'files'})` : action.label"
          :disabled="!props.hasRepo"
          @click="run(action)"
        >
          <component :is="action.icon" />
          <span :class="CAPTION_CLASS">{{ action.caption }}</span>
          <span v-if="badge(action)" :class="BADGE_CLASS" aria-hidden="true">{{ badge(action) }}</span>
        </button>
        <Tooltip v-else>
          <TooltipTrigger as-child>
            <!-- A disabled button swallows pointer events, so the wrapper span carries the tooltip. -->
            <span class="inline-flex">
              <button type="button" :class="buttonClass(action)" :aria-label="action.label" disabled>
                <component :is="action.icon" />
                <span :class="CAPTION_CLASS">{{ action.caption }}</span>
              </button>
            </span>
          </TooltipTrigger>
          <TooltipContent side="bottom" class="max-w-72">{{ action.label }}: {{ blocked(action) }}</TooltipContent>
        </Tooltip>
      </template>
    </template>
  </header>
</template>
