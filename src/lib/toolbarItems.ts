/**
 * The items the toolbar along the top of the window can hold, and the layout it starts with.
 * The layout is saved in `settings.json` as a list of item ids (`toolbar`); null there means
 * the default layout. `AppToolbar.vue` draws it, and the Toolbar page in Settings edits it.
 *
 * Besides the commands there are two layout items: a divider, the thin vertical rule between
 * groups of buttons, and a flexible spacer, which takes the free width and so pushes the items
 * after it to the right. Those two may appear any number of times; each command appears once.
 */
import type { Component } from "vue";
import {
  ArchiveIcon,
  ArchiveRestoreIcon,
  ArrowDownToLineIcon,
  ArrowDownUpIcon,
  ArrowUpFromLineIcon,
  CloudDownloadIcon,
  FolderDownIcon,
  FolderOpenIcon,
  GitBranchPlusIcon,
  GitCommitHorizontalIcon,
  GitMergeIcon,
  ListMinusIcon,
  ListPlusIcon,
  MoveHorizontalIcon,
  PanelBottomIcon,
  PanelLeftIcon,
  RefreshCwIcon,
  RotateCcwIcon,
  SearchIcon,
  SeparatorVerticalIcon,
  SettingsIcon,
  TagIcon,
  Undo2Icon,
} from "@lucide/vue";
import { dispatch } from "@/lib/menuActions";
import {
  applyStashReason,
  busyReason,
  discardSelected,
  fetchAll,
  fileActionReason,
  mergeRebaseReason,
  openApplyStashDialog,
  openCommitDialog,
  openMergeDialog,
  openPullDialog,
  openRebaseDialog,
  openStashDialog,
  pushCurrentBranch,
  stageSelected,
  syncCurrentBranch,
  unstageSelected,
} from "@/lib/repoActions";
import { useDialogsStore } from "@/stores/dialogs";
import { useSessionsStore, type RepoSession } from "@/stores/sessions";
import { useUiStateStore } from "@/stores/uiState";

/** The id of the thin vertical rule between groups of buttons. */
export const DIVIDER = "divider";
/** The id of the flexible spacer, which takes the free width of the toolbar. */
export const SPACER = "spacer";

/** How the Toolbar page in Settings names and explains the two layout items. */
export const LAYOUT_ITEMS: readonly { id: string; label: string; help: string; icon: Component }[] = [
  { id: DIVIDER, label: "Divider", help: "A thin vertical rule between groups of buttons.", icon: SeparatorVerticalIcon },
  { id: SPACER, label: "Flexible Spacer", help: "Takes the free width, so the items after it sit at the right-hand end.", icon: MoveHorizontalIcon },
];

export interface ToolbarCommand {
  /** Stable id, saved in `settings.json`. Never rename one: a saved layout would lose the button. */
  id: string;
  /** The full name: the button's tooltip, and its name on the Toolbar page in Settings. */
  label: string;
  /** Short caption under the icon; a button is 63px wide, so long labels are cut down. */
  caption: string;
  icon: Component;
  /** The main action: a solid primary button among the flat ones. */
  primary?: boolean;
  /** Acts on the active repository, so the button is disabled while no repository tab is open. */
  needsRepo: boolean;
  /** Runs the command against whatever is active when it is clicked. */
  run: () => unknown;
  /**
   * Why the command cannot act on `session` right now, or null when it can. A button with a
   * reason is disabled and shows the reason as its tooltip. Commands without this stay enabled
   * and explain in the notice strip when clicked, since their checks need more than the session.
   */
  reason?: (session: RepoSession) => string | null;
}

function active(): RepoSession | null {
  return useSessionsStore().active;
}

/** Every command the toolbar can hold, in the order the Toolbar page in Settings lists them. */
export const TOOLBAR_COMMANDS: readonly ToolbarCommand[] = [
  { id: "open", label: "Open Repository", caption: "Open", icon: FolderOpenIcon, needsRepo: false, run: () => useDialogsStore().showAddRepo() },
  { id: "clone", label: "Clone Repository…", caption: "Clone", icon: FolderDownIcon, needsRepo: false, run: () => useDialogsStore().cloneRepo() },
  {
    id: "refresh",
    label: "Refresh",
    caption: "Refresh",
    icon: RefreshCwIcon,
    needsRepo: false,
    run: () => {
      const sessions = useSessionsStore();
      if (sessions.activeKey !== null) return sessions.refreshSession(sessions.activeKey);
    },
  },
  // Greyed out while any fetch, pull or push runs, which would refuse a second; the one running
  // shows a spinner (`RunningOperation.command`).
  { id: "fetch", label: "Fetch from Every Remote", caption: "Fetch", icon: CloudDownloadIcon, needsRepo: true, run: () => fetchAll(active()), reason: busyReason },
  { id: "pull", label: "Pull", caption: "Pull", icon: ArrowDownToLineIcon, needsRepo: true, run: () => openPullDialog(active()), reason: busyReason },
  { id: "sync", label: "Sync: Pull, then Push", caption: "Sync", icon: ArrowDownUpIcon, needsRepo: true, run: () => syncCurrentBranch(active()), reason: busyReason },
  { id: "push", label: "Push to Upstream", caption: "Push", icon: ArrowUpFromLineIcon, needsRepo: true, run: () => pushCurrentBranch(active()), reason: busyReason },
  { id: "commit", label: "Commit", caption: "Commit", icon: GitCommitHorizontalIcon, primary: true, needsRepo: true, run: () => openCommitDialog(active()) },
  {
    id: "stage",
    label: "Stage the Selected Files",
    caption: "Stage",
    icon: ListPlusIcon,
    needsRepo: true,
    run: () => stageSelected(active()),
    reason: (s) => fileActionReason(s, "stage"),
  },
  {
    id: "unstage",
    label: "Unstage the Selected Files",
    caption: "Unstage",
    icon: ListMinusIcon,
    needsRepo: true,
    run: () => unstageSelected(active()),
    reason: (s) => fileActionReason(s, "unstage"),
  },
  {
    id: "discard",
    label: "Discard Changes to the Selected Files…",
    caption: "Discard",
    icon: Undo2Icon,
    needsRepo: true,
    run: () => discardSelected(active()),
    reason: (s) => fileActionReason(s, "discard"),
  },
  { id: "stash", label: "Stash Changes", caption: "Stash", icon: ArchiveIcon, needsRepo: true, run: () => openStashDialog(active()) },
  {
    id: "applyStash",
    label: "Apply Stash…",
    caption: "Apply",
    icon: ArchiveRestoreIcon,
    needsRepo: true,
    run: () => openApplyStashDialog(active()),
    reason: applyStashReason,
  },
  {
    id: "merge",
    label: "Merge a Branch into the Current Branch…",
    caption: "Merge",
    icon: GitMergeIcon,
    needsRepo: true,
    run: () => openMergeDialog(active()),
    reason: (s) => mergeRebaseReason(s, "merge"),
  },
  {
    id: "rebase",
    label: "Rebase the Current Branch onto Another…",
    caption: "Rebase",
    icon: RotateCcwIcon,
    needsRepo: true,
    run: () => openRebaseDialog(active()),
    reason: (s) => mergeRebaseReason(s, "rebase"),
  },
  // Branch and Tag start at the checked-out commit, as Repository > Create Branch and Create Tag do.
  { id: "createBranch", label: "Create Branch…", caption: "Branch", icon: GitBranchPlusIcon, needsRepo: true, run: () => dispatch("repo.create_branch") },
  { id: "createTag", label: "Create Tag…", caption: "Tag", icon: TagIcon, needsRepo: true, run: () => dispatch("repo.create_tag") },
  { id: "find", label: "Find Commit…", caption: "Find", icon: SearchIcon, needsRepo: true, run: () => useSessionsStore().openFind() },
  {
    id: "toggleSidebar",
    label: "Toggle Sidebar",
    caption: "Sidebar",
    icon: PanelLeftIcon,
    needsRepo: false,
    run: () => {
      const uiState = useUiStateStore();
      uiState.setSidebarCollapsed(!uiState.sidebarCollapsed);
    },
  },
  {
    id: "toggleConsole",
    label: "Toggle Console",
    caption: "Console",
    icon: PanelBottomIcon,
    needsRepo: false,
    run: () => {
      const uiState = useUiStateStore();
      uiState.setConsoleCollapsed(!uiState.consoleCollapsed);
    },
  },
  { id: "settings", label: "Settings…", caption: "Settings", icon: SettingsIcon, needsRepo: false, run: () => useDialogsStore().open("settings", {}) },
];

const COMMANDS_BY_ID: ReadonlyMap<string, ToolbarCommand> = new Map(TOOLBAR_COMMANDS.map((c) => [c.id, c]));

/** The command with `id`, or undefined for a layout item or an id this version does not know. */
export function toolbarCommand(id: string): ToolbarCommand | undefined {
  return COMMANDS_BY_ID.get(id);
}

/** The toolbar as it was before it could be customised, which is what a new or older settings file gets. */
export const DEFAULT_TOOLBAR: readonly string[] = [
  "open",
  "refresh",
  DIVIDER,
  "fetch",
  "pull",
  "sync",
  "push",
  "commit",
  DIVIDER,
  "stage",
  "unstage",
  "discard",
  DIVIDER,
  // No Apply Stash button by default: applying means choosing which stash, which the menus and
  // the Branches panel do. Saving always makes a new stash, so it can be one click.
  "stash",
  DIVIDER,
  "merge",
  "rebase",
];

/** One item of the toolbar as drawn, with a `key` that is unique within the layout. */
export type ToolbarEntry =
  | { kind: "command"; key: string; command: ToolbarCommand }
  | { kind: "divider"; key: string }
  | { kind: "spacer"; key: string };

/**
 * The layout to draw for a saved list of ids: the default layout when nothing is saved, and
 * otherwise the saved list without ids this version does not know (a command from a newer
 * version, or a typing mistake in a hand-edited file) and without a command's second and later
 * appearances. Dividers and spacers are kept wherever they are.
 */
export function normaliseToolbar(saved: readonly string[] | null | undefined): string[] {
  if (!saved) return [...DEFAULT_TOOLBAR];
  const seen = new Set<string>();
  const result: string[] = [];
  for (const id of saved) {
    if (id === DIVIDER || id === SPACER) {
      result.push(id);
    } else if (COMMANDS_BY_ID.has(id) && !seen.has(id)) {
      seen.add(id);
      result.push(id);
    }
  }
  return result;
}

/** The saved list of ids as items to draw, in order. See `normaliseToolbar` for what is dropped. */
export function resolveToolbar(saved: readonly string[] | null | undefined): ToolbarEntry[] {
  return normaliseToolbar(saved).map((id, index): ToolbarEntry => {
    if (id === DIVIDER) return { kind: "divider", key: `${DIVIDER}-${index}` };
    if (id === SPACER) return { kind: "spacer", key: `${SPACER}-${index}` };
    return { kind: "command", key: id, command: COMMANDS_BY_ID.get(id)! };
  });
}

/** The commands that are not in `layout` yet, in registry order: what the Toolbar page offers to add. */
export function availableCommands(layout: readonly string[]): ToolbarCommand[] {
  const used = new Set(layout);
  return TOOLBAR_COMMANDS.filter((c) => !used.has(c.id));
}

/** `layout` with the item at `from` moved to `to`. An index out of range leaves it unchanged. */
export function moveToolbarItem(layout: readonly string[], from: number, to: number): string[] {
  const result = [...layout];
  if (from < 0 || from >= result.length || to < 0 || to >= result.length || from === to) return result;
  const [item] = result.splice(from, 1);
  result.splice(to, 0, item!);
  return result;
}

/** True when `layout` is the default layout, which is then saved as null so a later default reaches it. */
export function isDefaultToolbar(layout: readonly string[]): boolean {
  return layout.length === DEFAULT_TOOLBAR.length && layout.every((id, i) => id === DEFAULT_TOOLBAR[i]);
}
