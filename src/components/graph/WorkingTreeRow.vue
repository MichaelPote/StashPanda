<script setup lang="ts">
import { computed } from "vue";
import type { GraphRow } from "@/lib/bindings";
import { cn } from "@/lib/utils";

const props = defineProps<{
  row: GraphRow;
  selected: boolean;
}>();

const badges = computed(() => {
  const wt = props.row.worktree;
  if (!wt) return [];
  const list: { label: string; count: number; classes: string }[] = [
    { label: "staged", count: wt.staged, classes: "badge-primary" },
    { label: "unstaged", count: wt.unstaged, classes: "badge-modified" },
    { label: "untracked", count: wt.untracked, classes: "badge-untracked" },
    { label: "conflicted", count: wt.conflicted, classes: "badge-conflict" },
  ];
  return list.filter((b) => b.count > 0);
});
</script>

<template>
  <div
    :class="cn('graph-grid graph-row h-(--row-h) text-base', selected ? 'row-selected' : 'hover:bg-row-hover')"
    :data-index="row.index"
    role="row"
    :aria-selected="selected"
  >
    <div />
    <div class="flex min-w-0 items-center gap-1.5 overflow-hidden pr-2 leading-(--row-h)">
      <span class="shrink-0 italic">Working Tree</span>
      <span
        v-for="b in badges"
        :key="b.label"
        :class="cn('inline-flex h-(--pill-h) shrink-0 items-center rounded-sm px-1 text-[0.7rem] leading-[1.3] tabular-nums', b.classes)"
        :title="`${b.count} ${b.label}`"
      >
        {{ b.count }} {{ b.label }}
      </span>
      <span v-if="row.worktree && badges.length === 0" class="shrink-0 text-muted-foreground">No changes</span>
    </div>
    <div />
    <div />
    <div />
  </div>
</template>
