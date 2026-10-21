import { computed, type ComputedRef } from "vue";
import { useSettingsStore } from "@/stores/settings";

/** A commit list column that can be hidden; the commit message always shows. */
export type HideableColumn = "graph" | "author" | "date" | "id";

/** Each column's setting in `appearance`, and its name in the column menus. */
export const COMMIT_COLUMNS: { column: HideableColumn; key: "showGraphColumn" | "showAuthorColumn" | "showDateColumn" | "showIdColumn"; label: string }[] = [
  { column: "graph", key: "showGraphColumn", label: "Graph" },
  { column: "author", key: "showAuthorColumn", label: "Author" },
  { column: "date", key: "showDateColumn", label: "Date" },
  { column: "id", key: "showIdColumn", label: "ID" },
];

/**
 * Which of the commit list's columns show, from `appearance.show…Column` (all of them until the
 * settings load), and `setShown` to change one at once, as the column headings' menu does.
 */
export function useCommitColumns(): {
  shown: ComputedRef<Record<HideableColumn, boolean>>;
  setShown: (column: HideableColumn, show: boolean) => Promise<void>;
} {
  const settings = useSettingsStore();
  const shown = computed(() => {
    const appearance = settings.settings?.appearance;
    return {
      graph: appearance?.showGraphColumn ?? true,
      author: appearance?.showAuthorColumn ?? true,
      date: appearance?.showDateColumn ?? true,
      id: appearance?.showIdColumn ?? true,
    };
  });
  async function setShown(column: HideableColumn, show: boolean): Promise<void> {
    const key = COMMIT_COLUMNS.find((c) => c.column === column)!.key;
    await settings.update({ appearance: { [key]: show } });
  }
  return { shown, setShown };
}
