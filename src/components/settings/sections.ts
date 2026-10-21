/**
 * What the Settings dialog shows: one entry per page in its left-hand list, each holding the
 * fields of `settings.json` it edits. `SettingsDialog.vue` renders these; `SettingsField.vue`
 * renders one field.
 *
 * Settings that only show or hide something are left out when the window already has a control
 * for it: `appearance.showRemoteBranches` (each branch's show/hide tag in the Branches panel),
 * `console.showInternal` (the Console header's Internal button) and
 * `appearance.showStashesInGraph` (belongs on the Branches panel's Stashes heading). The diff
 * viewer's header controls (layout, context lines, whitespace) are left out for the same reason.
 */
import type { Component } from "vue";
import { CpuIcon, DatabaseIcon, FileDiffIcon, GitCommitHorizontalIcon, KeyRoundIcon, PanelTopIcon, SlidersHorizontalIcon, TerminalIcon, TimerIcon } from "@lucide/vue";
import type { Settings } from "@/lib/bindings";
import DiffPreview from "./DiffPreview.vue";
import KeyManagerPage from "./KeyManagerPage.vue";
import ToolbarPage from "./ToolbarPage.vue";

/** The sections of `settings.json` the dialog edits. `version` and `library` are left out. */
export type SettingsGroup = "git" | "appearance" | "console" | "polling" | "cache" | "commit" | "diff";

/** What a field holds while the dialog is open: the text in its box, or a toggle's state. */
export type FormValue = string | boolean;

interface FieldBase {
  /** Where the value lives: `settings[group][key]`. */
  group: SettingsGroup;
  key: string;
  label: string;
  help?: string;
  /**
   * When a saved change takes effect, if not straight away. `unused` means nothing reads the
   * setting yet; `reopen` means it is read when a repository is opened.
   */
  applies?: "unused" | "reopen";
}

export interface SelectField extends FieldBase {
  kind: "select";
  options: { value: string; label: string }[];
}

export interface ToggleField extends FieldBase {
  kind: "toggle";
  /**
   * Toggles next to each other with the same `list` are drawn as one list of tickboxes under that
   * heading. The first one's `listHelp` goes under the list.
   */
  list?: string;
  listHelp?: string;
}

/** A list of the bundled monospaced fonts and those installed on this machine. */
export interface FontField extends FieldBase {
  kind: "font";
}

export interface NumberField extends FieldBase {
  kind: "number";
  /** The unit shown after the box, in the scale shown (seconds, not milliseconds). */
  unit?: string;
  /** Stored value = shown value × scale, so 1000 shows milliseconds as seconds. */
  scale?: number;
  /** Limits on the shown value. */
  min: number;
  max: number;
  step?: number;
  /** An empty box saves null (the setting is off); otherwise empty is an error. */
  optional?: boolean;
  /** Placeholder for an optional box left empty. */
  emptyLabel?: string;
}

export interface TextField extends FieldBase {
  kind: "text";
  placeholder?: string;
  monospace?: boolean;
  /** An empty box saves null (discover or use the default); otherwise empty is an error. */
  optional?: boolean;
}

export type SettingsFieldDef = SelectField | ToggleField | NumberField | TextField | FontField;

export interface SettingsSection {
  id: string;
  label: string;
  icon: Component;
  description: string;
  fields: SettingsFieldDef[];
  /** Drawn under the fields with the form's current values, before they are saved. */
  preview?: Component;
  /**
   * Drawn in place of the field list, for a page that is not a form of settings (the SSH Keys
   * page). Its changes take effect at once rather than on Save Changes.
   */
  page?: Component;
}

export const SETTINGS_SECTIONS: SettingsSection[] = [
  {
    id: "general",
    label: "Appearance",
    icon: SlidersHorizontalIcon,
    description: "The look of the window and how the commit graph is drawn.",
    fields: [
      {
        kind: "select",
        group: "appearance",
        key: "theme",
        label: "Theme",
        help: "System follows the Windows or desktop light and dark setting.",
        options: [
          { value: "system", label: "System" },
          { value: "dark", label: "Dark" },
          { value: "light", label: "Light" },
        ],
      },
      {
        kind: "number",
        group: "appearance",
        key: "zoom",
        label: "Zoom",
        help: "Draws the whole window larger or smaller.",
        unit: "%",
        min: 50,
        max: 200,
        step: 10,
      },
      {
        kind: "select",
        group: "appearance",
        key: "dateFormat",
        label: "Dates",
        help: "How commit dates appear in the graph and the Files panel.",
        options: [
          { value: "relative", label: "Relative (3 hours ago)" },
          { value: "absolute", label: "Absolute (2026-09-30 14:05)" },
        ],
      },
      {
        kind: "select",
        group: "appearance",
        key: "graphOrder",
        label: "Graph order",
        help: "Topological keeps each branch's commits together, even when their dates interleave.",
        options: [
          { value: "date", label: "By date" },
          { value: "topo", label: "Topological" },
        ],
      },
      {
        kind: "select",
        group: "appearance",
        key: "commitLaneColours",
        label: "Lane colours",
        help: "How a lane is coloured when it carries several branch tips, as when one branch was started from the tip of another.",
        options: [
          { value: "keepLane", label: "Keep one colour per lane" },
          { value: "changeAtEachTip", label: "Change colour at each branch tip" },
        ],
      },
      {
        kind: "toggle",
        group: "appearance",
        key: "solidRemotePills",
        label: "Solid remote branch pills",
        help: "Draw remote branches solid and local branches outlined in the commit graph, instead of the other way round. Tags stay solid.",
      },
      {
        kind: "toggle",
        group: "appearance",
        key: "graphInMessageColumn",
        label: "Graph inside the commit message column",
        help: "Draw the lanes at the start of the commit message column instead of in a column of their own, so each message starts just past the lanes on its row.",
      },
      {
        kind: "toggle",
        group: "appearance",
        key: "showColumnHeadings",
        label: "Column headings above the commits",
        help: "The Commit message, Author, Date and ID headings. The columns can only be resized while they show, since their resize handles are in the headings.",
      },
      {
        kind: "select",
        group: "appearance",
        key: "avatarSource",
        label: "Author pictures",
        help: "Gravatar sends a hash of each author's email address to gravatar.com.",
        options: [
          { value: "initials", label: "Initials" },
          { value: "gravatar", label: "Gravatar" },
        ],
      },
      {
        kind: "toggle",
        group: "appearance",
        key: "authorAvatarOnly",
        label: "Author pictures only",
        help: "Show only each author's picture or initials in the commit list, without the name. Hover over a picture to see the name and email address.",
      },
      {
        kind: "toggle",
        group: "appearance",
        key: "showGraphColumn",
        label: "Graph",
        list: "Commit columns",
        listHelp:
          "Which columns the commit list shows. The commit message always shows, and takes the room the others leave. Right-click the column headings to change them there too.",
      },
      { kind: "toggle", group: "appearance", key: "showAuthorColumn", label: "Author", list: "Commit columns" },
      { kind: "toggle", group: "appearance", key: "showDateColumn", label: "Date", list: "Commit columns" },
      { kind: "toggle", group: "appearance", key: "showIdColumn", label: "ID", list: "Commit columns" },
    ],
  },
  {
    id: "toolbar",
    label: "Toolbar",
    icon: PanelTopIcon,
    description: "The buttons along the top of the window. The toolbar behind this dialog shows your changes as you make them; Save Changes keeps them, and Cancel puts it back.",
    fields: [],
    page: ToolbarPage,
  },
  {
    id: "commit",
    label: "Commit",
    icon: GitCommitHorizontalIcon,
    description: "How the Commit dialog starts.",
    fields: [
      {
        kind: "select",
        group: "commit",
        key: "preselect",
        label: "Files ticked when nothing is selected",
        help: "When files are selected in the Files panel, the Commit dialog ticks those instead.",
        options: [
          { value: "all", label: "Every changed file" },
          { value: "staged", label: "Only staged files" },
        ],
      },
    ],
  },
  {
    id: "diff",
    label: "Diff",
    icon: FileDiffIcon,
    description: "How the diff viewer draws its text. The preview below shows the values before you save them.",
    preview: DiffPreview,
    fields: [
      {
        kind: "font",
        group: "diff",
        key: "fontFamily",
        label: "Font",
        help: "JetBrains Mono and IBM Plex Mono come with StashPanda. The others are the monospaced fonts installed on this computer.",
      },
      {
        kind: "number",
        group: "diff",
        key: "fontSize",
        label: "Font size",
        help: "The rest of the app uses 14px text; the default diff size is 12px.",
        unit: "px",
        min: 6,
        max: 48,
        step: 0.5,
      },
      {
        kind: "number",
        group: "diff",
        key: "lineHeight",
        label: "Line height",
        help: "Each row's height as a multiple of the font size; 1.2 leaves 20% of space around the text.",
        unit: "× font size",
        min: 1,
        max: 3,
        step: 0.05,
      },
      {
        kind: "number",
        group: "diff",
        key: "tabWidth",
        label: "Tab width",
        help: "How many columns a tab character moves the text along.",
        unit: "columns",
        min: 1,
        max: 16,
      },
      {
        kind: "toggle",
        group: "diff",
        key: "ligatures",
        label: "Use font ligatures",
        help: "Fonts that have them draw pairs such as => and != as one symbol. Turn this off to see every character as typed.",
      },
      {
        kind: "select",
        group: "diff",
        key: "fontSmoothing",
        label: "Font smoothing",
        help: "How the diff text's edges are drawn. ClearType is the sharpest, using the screen's red, green and blue subpixels; it needs ClearType switched on in Windows. Grayscale is softer, with no colour fringes. None draws every pixel as text or background, for hard bitmap-like edges. Only the diff text changes; the rest of the interface keeps the Windows setting.",
        options: [
          { value: "clearType", label: "ClearType" },
          { value: "grayscale", label: "Grayscale" },
          { value: "none", label: "None" },
        ],
      },
      {
        kind: "toggle",
        group: "diff",
        key: "wordHighlight",
        label: "Highlight changed words",
        help: "Mark the words that changed inside a changed line, as well as colouring the whole line.",
      },
      {
        kind: "toggle",
        group: "diff",
        key: "syntaxHighlight",
        label: "Syntax Highlighting",
        help: "Colour keywords, strings and comments by the file's language, chosen from its extension. Files without a known extension stay plain.",
      },
      {
        kind: "toggle",
        group: "diff",
        key: "matchSimilarLines",
        label: "Match similar lines",
        help: "In a block of changed lines, compare each removed line with the added line most like it, not the one in the same position. Side by side, each matched pair gets a curve of its own. Off pairs the lines in the order Git lists them.",
      },
    ],
  },
  {
    id: "git",
    label: "Git executables",
    icon: CpuIcon,
    description: "Which git program runs commands. Leave a path empty to find git on the PATH. Changes apply when a repository is opened.",
    fields: [
      {
        kind: "text",
        group: "git",
        key: "windowsExecutable",
        label: "git.exe for the Windows build",
        placeholder: "C:\\Program Files\\Git\\cmd\\git.exe",
        monospace: true,
        optional: true,
      },
      {
        kind: "text",
        group: "git",
        key: "linuxExecutable",
        label: "git for the Linux build",
        placeholder: "/usr/bin/git",
        monospace: true,
        optional: true,
      },
      {
        kind: "text",
        group: "git",
        key: "windowsExecutableFromLinux",
        label: "Windows git.exe, seen from Linux",
        help: "Used for repositories under /mnt/c and the other Windows drives.",
        placeholder: "/mnt/c/Program Files/Git/cmd/git.exe",
        monospace: true,
        optional: true,
      },
      {
        kind: "text",
        group: "git",
        key: "wslDefaultDistro",
        label: "Default WSL distribution",
        help: "Used when a \\\\wsl$ path does not name a distribution.",
        placeholder: "Ubuntu",
        optional: true,
      },
      {
        kind: "text",
        group: "git",
        key: "wslAutomountRoot",
        label: "WSL automount root",
        help: "Where WSL mounts the Windows drives. Leave empty to use /mnt; fill in only if /etc/wsl.conf sets a different root.",
        placeholder: "/mnt",
        monospace: true,
        // The backend stores "" by default and reads an empty value as /mnt (`repo/resolve.rs`).
        optional: true,
      },
    ],
  },
  {
    id: "sshKeys",
    label: "SSH Keys",
    icon: KeyRoundIcon,
    description:
      "Every private key in your .ssh folders, in any format StashPanda reads. Its actions take effect straight away, without Save Changes.",
    fields: [],
    page: KeyManagerPage,
  },
  {
    id: "console",
    label: "Console",
    icon: TerminalIcon,
    description: "What the console panel records.",
    fields: [
      {
        kind: "toggle",
        group: "console",
        key: "verboseHelpers",
        label: "Log requests to helper processes",
        help: "Each request sent to the long-running git cat-file and inotifywait processes.",
        applies: "reopen",
      },
      {
        kind: "number",
        group: "console",
        key: "maxLines",
        label: "Lines kept per repository",
        unit: "lines",
        min: 100,
        max: 100_000,
        step: 100,
      },
      {
        kind: "number",
        group: "console",
        key: "outputCapBytes",
        label: "Output kept per command",
        help: "Standard output and standard error beyond this size are cut off.",
        unit: "KiB",
        scale: 1024,
        min: 1,
        max: 16_384,
        applies: "reopen",
      },
    ],
  },
  {
    id: "polling",
    label: "Polling",
    icon: TimerIcon,
    description: "How often StashPanda checks repositories for changes made outside it.",
    fields: [
      {
        kind: "number",
        group: "polling",
        key: "statusIntervalMs",
        label: "Working tree, active tab",
        help: "Used where file-change notifications are not available, such as Windows drives seen from WSL.",
        unit: "seconds",
        scale: 1000,
        min: 1,
        max: 600,
        step: 0.5,
        applies: "reopen",
      },
      {
        kind: "number",
        group: "polling",
        key: "statusBackgroundIntervalMs",
        label: "Working tree, background tabs",
        unit: "seconds",
        scale: 1000,
        min: 1,
        max: 3600,
        step: 0.5,
        applies: "reopen",
      },
      {
        kind: "number",
        group: "polling",
        key: "refsIntervalMs",
        label: "Branches and tags",
        unit: "seconds",
        scale: 1000,
        min: 1,
        max: 3600,
        step: 0.5,
        applies: "reopen",
      },
      {
        kind: "number",
        group: "polling",
        key: "autoFetchMinutes",
        label: "Fetch from remotes every",
        unit: "minutes",
        min: 1,
        max: 1440,
        optional: true,
        emptyLabel: "Off",
        help: "Every open repository fetches from all its remotes this often. A failure is reported once, until a fetch works again.",
      },
    ],
  },
  {
    id: "cache",
    label: "Cache",
    icon: DatabaseIcon,
    description: "How much history StashPanda keeps on disk between runs.",
    fields: [
      {
        kind: "number",
        group: "cache",
        key: "maxCommitsPerRepo",
        label: "Commits loaded per repository",
        help: "Older history is not shown in the graph.",
        unit: "commits",
        min: 1000,
        max: 10_000_000,
        step: 1000,
      },
      {
        kind: "number",
        group: "cache",
        key: "maxCachedRepos",
        label: "Repositories kept in the cache",
        help: "Checked when StashPanda starts: the repositories opened least recently beyond this number lose their cached history and load it from Git again next time.",
        unit: "repositories",
        min: 1,
        max: 1000,
      },
    ],
  },
];

/** The key a field's form value is kept under: `appearance.theme`. */
export function fieldId(field: SettingsFieldDef): string {
  return `${field.group}.${field.key}`;
}

/** Reads a field's stored value from the settings. */
export function readStored(settings: Settings, field: SettingsFieldDef): unknown {
  return (settings[field.group] as Record<string, unknown>)[field.key];
}

/** What the form shows for a stored value: text for boxes and lists, true or false for toggles. */
export function toFormValue(field: SettingsFieldDef, stored: unknown): FormValue {
  switch (field.kind) {
    case "toggle":
      return stored === true;
    case "select":
    case "font":
      return String(stored ?? "");
    case "text":
      return typeof stored === "string" ? stored : "";
    case "number": {
      if (typeof stored !== "number") return "";
      // Rounding hides float noise such as 1.2000000476837158 from an f32 setting.
      return String(Math.round((stored / (field.scale ?? 1)) * 10_000) / 10_000);
    }
  }
}

/** The stored value for what the form holds, or the reason it cannot be saved. */
export function fromFormValue(field: SettingsFieldDef, value: FormValue): { value: unknown } | { error: string } {
  switch (field.kind) {
    case "toggle":
      return { value: value === true };
    case "select":
      return { value };
    case "font": {
      const name = String(value).trim();
      return name.length > 0 ? { value: name } : { error: "Choose a font." };
    }
    case "text": {
      const text = String(value).trim();
      if (text.length > 0) return { value: text };
      return field.optional ? { value: null } : { error: "This cannot be empty." };
    }
    case "number": {
      const text = String(value).trim();
      if (text.length === 0) return field.optional ? { value: null } : { error: "Enter a number." };
      const shown = Number(text);
      const range = `Enter a number from ${field.min.toLocaleString()} to ${field.max.toLocaleString()}.`;
      if (!Number.isFinite(shown) || shown < field.min || shown > field.max) return { error: range };
      if (field.scale !== undefined) return { value: Math.round(shown * field.scale) };
      // Without a scale, only a field with a fractional step takes fractions (diffLineHeight).
      if (Number.isInteger(field.step ?? 1)) {
        return Number.isInteger(shown) ? { value: shown } : { error: "Enter a whole number." };
      }
      return { value: shown };
    }
  }
}
