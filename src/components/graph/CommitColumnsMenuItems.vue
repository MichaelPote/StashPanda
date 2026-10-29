<script setup lang="ts">
import { computed } from "vue";
import { ContextMenuCheckboxItem, ContextMenuLabel, ContextMenuSeparator } from "@/components/ui/context-menu";
import { COMMIT_COLUMNS, useCommitColumns } from "@/composables/useCommitColumns";
import { useSettingsStore } from "@/stores/settings";

/**
 * The commit list's column switches, and whether its column headings show: the right-click menu of
 * the headings and of the Commits pane's header, which stays when the headings are hidden.
 */
const { shown, setShown } = useCommitColumns();
const settings = useSettingsStore();
const headings = computed(() => settings.settings?.appearance?.showColumnHeadings ?? true);

function setHeadings(show: boolean): void {
  void settings.update({ appearance: { showColumnHeadings: show } });
}
</script>

<template>
  <ContextMenuLabel>Columns</ContextMenuLabel>
  <ContextMenuCheckboxItem
    v-for="c in COMMIT_COLUMNS"
    :key="c.column"
    :model-value="shown[c.column]"
    @update:model-value="(v: boolean | 'indeterminate') => setShown(c.column, v === true)"
    @select.prevent
  >
    {{ c.label }}
  </ContextMenuCheckboxItem>
  <ContextMenuSeparator />
  <ContextMenuCheckboxItem :model-value="headings" data-column-headings @update:model-value="(v: boolean | 'indeterminate') => setHeadings(v === true)" @select.prevent>
    Column Headings
  </ContextMenuCheckboxItem>
</template>
