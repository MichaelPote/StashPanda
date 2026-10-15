/**
 * The palette's twelve hues, which branch, tag and repository colours are chosen from: palette
 * indices 0 to 11 (`--graph-lane-0` to `--graph-lane-11`), every colour the graph draws. The
 * light theme draws darker versions of the same twelve as the dark theme, so a hue has the same
 * name in both.
 */
import { computed, type ComputedRef } from "vue";

export const HUES = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11] as const;

const NAMES = ["Violet", "Cyan", "Green", "Yellow", "Orange", "Magenta", "Amber", "Blue", "Lime", "Orchid", "Mint", "Rose"];

/** The hues' names, by palette index. */
export function useHueNames(): ComputedRef<string[]> {
  return computed(() => NAMES);
}
