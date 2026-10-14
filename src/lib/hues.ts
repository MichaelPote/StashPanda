/**
 * The palette's six hues, which branch, tag and repository colours are chosen from: palette
 * indices 0 to 5 (`--graph-lane-0` to `--graph-lane-5`). The dark theme repeats them as 6 to 11,
 * and the light theme's 6 to 11 are shades of the same hues, so these six are every colour there
 * is to choose. Each theme draws them differently, so their names depend on the theme.
 */
import { computed, type ComputedRef } from "vue";
import { usePreferredDark } from "@vueuse/core";
import { useSettingsStore } from "@/stores/settings";

export const HUES = [0, 1, 2, 3, 4, 5] as const;

const DARK_NAMES = ["Violet", "Teal", "Green", "Yellow", "Orange", "Pink"];
const LIGHT_NAMES = ["Gold", "Teal", "Orange", "Purple", "Red", "Grey"];

/** The hues' names in the theme shown now. */
export function useHueNames(): ComputedRef<string[]> {
  const settings = useSettingsStore();
  const preferredDark = usePreferredDark();
  return computed(() => (settings.theme === "dark" || (settings.theme === "system" && preferredDark.value) ? DARK_NAMES : LIGHT_NAMES));
}
