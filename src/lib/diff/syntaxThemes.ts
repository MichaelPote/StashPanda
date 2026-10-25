/**
 * The colour themes offered for code in the diff, the Blame dialog and the Merge editor: some of
 * Shiki's bundled themes, one list for each app theme, the default first. `syntax.worker.ts`
 * loads them, and the Diff page of Settings offers them. Only the text takes a theme's colours;
 * the background and the added and removed lines keep the app's own.
 */

export const DARK_SYNTAX_THEMES = [
  { id: "github-dark-default", label: "GitHub Dark" },
  { id: "dark-plus", label: "Dark+ (VS Code)" },
  { id: "dracula", label: "Dracula" },
  { id: "monokai", label: "Monokai" },
  { id: "one-dark-pro", label: "One Dark Pro" },
  { id: "nord", label: "Nord" },
  { id: "solarized-dark", label: "Solarized Dark" },
  { id: "night-owl", label: "Night Owl" },
  { id: "tokyo-night", label: "Tokyo Night" },
  { id: "catppuccin-mocha", label: "Catppuccin Mocha" },
  { id: "gruvbox-dark-medium", label: "Gruvbox Dark" },
  { id: "material-theme-darker", label: "Material Darker" },
  { id: "ayu-dark", label: "Ayu Dark" },
  { id: "rose-pine", label: "Rosé Pine" },
] as const;

export const LIGHT_SYNTAX_THEMES = [
  { id: "github-light-default", label: "GitHub Light" },
  { id: "light-plus", label: "Light+ (VS Code)" },
  { id: "solarized-light", label: "Solarized Light" },
  { id: "one-light", label: "One Light" },
  { id: "catppuccin-latte", label: "Catppuccin Latte" },
  { id: "gruvbox-light-medium", label: "Gruvbox Light" },
  { id: "material-theme-lighter", label: "Material Lighter" },
  { id: "ayu-light", label: "Ayu Light" },
  { id: "night-owl-light", label: "Night Owl Light" },
  { id: "rose-pine-dawn", label: "Rosé Pine Dawn" },
] as const;

export type SyntaxTheme = (typeof DARK_SYNTAX_THEMES)[number]["id"] | (typeof LIGHT_SYNTAX_THEMES)[number]["id"];
