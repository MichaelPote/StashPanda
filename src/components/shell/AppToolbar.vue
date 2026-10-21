<script setup lang="ts">
import { computed } from "vue";
import { LoaderCircleIcon } from "@lucide/vue";
import { ContextMenu, ContextMenuContent, ContextMenuItem, ContextMenuTrigger } from "@/components/ui/context-menu";
import { Separator } from "@/components/ui/separator";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { commitFileCount } from "@/lib/files";
import { resolveToolbar, type ToolbarCommand } from "@/lib/toolbarItems";
import { cn } from "@/lib/utils";
import { useDialogsStore } from "@/stores/dialogs";
import { useSessionsStore } from "@/stores/sessions";
import { useSettingsStore } from "@/stores/settings";

/**
 * The toolbar along the top of the window. Its items and their order come from the `toolbar`
 * setting, or the default layout when that is not set (`lib/toolbarItems.ts`). Right-clicking
 * anywhere on it offers Customise…, which opens the Toolbar page in Settings.
 */
const sessions = useSessionsStore();
const settings = useSettingsStore();
const dialogs = useDialogsStore();

// The Settings dialog's unsaved layout while one is being edited, so the change shows in place.
const entries = computed(() => resolveToolbar(settings.toolbarDraft ?? settings.settings?.toolbar));
/** False when no repository tab is open, which disables the buttons that act on a repository. */
const hasRepo = computed(() => sessions.active !== null);
/** Files the Commit dialog would list, badged on the Commit button; hidden at 0. */
const commitCount = computed(() => commitFileCount(sessions.active?.status ?? null));

function disabled(command: ToolbarCommand): boolean {
  return command.needsRepo && !hasRepo.value;
}

/** Why the button cannot be used right now, shown as its tooltip; null when it can, or when no repository is open. */
function blocked(command: ToolbarCommand): string | null {
  const session = sessions.active;
  if (!session || !command.reason) return null;
  return command.reason(session);
}

/** The operation `command` started, while it runs: its button shows a spinner, and its title as the tooltip. */
function running(command: ToolbarCommand): string | null {
  const op = sessions.active?.operation;
  return op && op.command === command.id ? `${op.title}…` : null;
}

function customise(): void {
  void dialogs.open("settings", { section: "toolbar" });
}

/**
 * Flat toolbar button: icon on top, caption below, no fill or border at rest. Hover and press
 * lay the ink over the bar at 6% and 10%; disabled drops to 38% opacity and lets the pointer
 * through to the wrapper behind it, which carries the tooltip and the right-click menu. The 20px
 * icons draw a 1.67px line (Lucide's 2 units at 20px) where the app's other icons draw 1px.
 */
const BUTTON_CLASS =
  "relative inline-flex h-[42px] w-[63px] shrink-0 flex-col items-center justify-between rounded-[4px] px-[2px] pt-[6px] pb-[5px] text-[10px] leading-none font-medium whitespace-nowrap transition-colors duration-[120ms] outline-none enabled:active:bg-foreground/10 focus-visible:ring-3 focus-visible:ring-ring/50 disabled:pointer-events-none disabled:opacity-38 [&_svg]:size-5 [&_svg]:shrink-0 [&_svg]:[stroke-width:1.67px]";
/** A flat button's ink and hover. Kept off the primary button: `cn` does not take the primary
    fill's text colour for one that replaces `text-foreground`, so both stayed and the flat one won. */
const FLAT_CLASS = "text-foreground/90 enabled:hover:bg-foreground/6 enabled:hover:text-foreground";
/** The primary button: solid primary fill, a step towards the primary colour on hover, darker pressed. */
const PRIMARY_CLASS =
  "bg-primary-fill text-primary-fill-foreground enabled:hover:bg-[color-mix(in_srgb,var(--primary-fill)_40%,var(--primary))] enabled:hover:text-primary-fill-foreground enabled:active:bg-[color-mix(in_srgb,var(--primary-fill)_85%,black)]";

function buttonClass(command: ToolbarCommand): string {
  return cn(BUTTON_CLASS, command.primary ? PRIMARY_CLASS : FLAT_CLASS);
}

/** Count badge in a button's top right corner, in the primary fill. */
const BADGE_CLASS =
  "absolute top-[3px] right-[3px] inline-flex h-[15px] items-center rounded-[3px] bg-primary-fill px-[3px] text-[9px] leading-none text-primary-fill-foreground tabular-nums";

/** The Commit button's badge text: the file count, capped at 999+; null for none or other buttons. */
function badge(command: ToolbarCommand): string | null {
  if (command.id !== "commit" || commitCount.value <= 0) return null;
  return commitCount.value > 999 ? "999+" : String(commitCount.value);
}

/** The tooltip's bold first line: the command's name, with the Commit button's file count. */
function title(command: ToolbarCommand): string {
  const count = badge(command);
  return count ? `${command.label} (${count} ${commitCount.value === 1 ? "file" : "files"})` : command.label;
}

/** The tooltip's last line, when the button cannot be used: what is running, or why not. */
function note(command: ToolbarCommand): string | null {
  if (disabled(command)) return "Open a repository first.";
  return running(command) ?? blocked(command);
}
const CAPTION_CLASS = "block max-w-[59px] truncate text-[9px] font-medium tracking-[0.04em] uppercase";
</script>

<template>
  <ContextMenu>
    <ContextMenuTrigger as-child>
      <header class="flex h-[52px] shrink-0 items-center border-b border-border bg-bar px-[6px] select-none">
        <template v-for="entry in entries" :key="entry.key">
          <!-- data-[orientation=vertical]:self-center replaces the Separator's own self-stretch, which
               would pin a fixed-height divider to the top of the bar. -->
          <Separator
            v-if="entry.kind === 'divider'"
            orientation="vertical"
            class="mx-[5px] h-[32px] bg-(--key) data-[orientation=vertical]:self-center"
          />
          <!-- Takes the free width, so the items after it sit at the right-hand end. -->
          <div v-else-if="entry.kind === 'spacer'" class="h-full w-full grow" aria-hidden="true" />
          <!-- Every button has the same tooltip: the name in bold, what it does, and, when it cannot
               be used, what is running or why not. A disabled button swallows pointer events, so
               the wrapper span carries the tooltip. -->
          <Tooltip v-else>
            <TooltipTrigger as-child>
              <span class="inline-flex shrink-0">
                <button
                  v-if="note(entry.command) === null"
                  type="button"
                  :class="buttonClass(entry.command)"
                  :aria-label="title(entry.command)"
                  @click="entry.command.run()"
                >
                  <component :is="entry.command.icon" />
                  <span :class="CAPTION_CLASS">{{ entry.command.caption }}</span>
                  <span v-if="badge(entry.command)" :class="BADGE_CLASS" aria-hidden="true">{{ badge(entry.command) }}</span>
                </button>
                <button
                  v-else
                  type="button"
                  :class="buttonClass(entry.command)"
                  :aria-label="entry.command.label"
                  :aria-busy="running(entry.command) !== null"
                  disabled
                >
                  <LoaderCircleIcon v-if="running(entry.command)" class="animate-spin" />
                  <component :is="entry.command.icon" v-else />
                  <span :class="CAPTION_CLASS">{{ entry.command.caption }}</span>
                </button>
              </span>
            </TooltipTrigger>
            <TooltipContent side="bottom" class="max-w-72">
              <!-- A column: the tooltip lays its content out in a row. -->
              <div class="flex flex-col gap-0.5">
                <span class="font-semibold">{{ title(entry.command) }}</span>
                <span>{{ entry.command.description }}</span>
                <span v-if="note(entry.command)" class="italic">{{ note(entry.command) }}</span>
              </div>
            </TooltipContent>
          </Tooltip>
        </template>
      </header>
    </ContextMenuTrigger>
    <ContextMenuContent class="min-w-44">
      <ContextMenuItem class="text-base" @select="customise">Customise…</ContextMenuItem>
    </ContextMenuContent>
  </ContextMenu>
</template>
