<script setup lang="ts">
import { computed } from "vue";
import { nameParts } from "@/lib/branchTree";

/**
 * A branch name that, in a row too narrow for it, shrinks from the left: each folder part
 * (`dependabot/`) is its own span that truncates to an ellipsis, the leftmost first, and the last
 * part keeps its full width until the folder parts have all given way. The row's tooltip still
 * has the whole name.
 */
const props = defineProps<{ name: string }>();

const parts = computed(() => nameParts(props.name));
/**
 * How hard each folder part shrinks: the leftmost the most, and every one far more than the last
 * part's 1. Flex layout shares the space a row lacks out by weight times width, so next to these
 * the last part gives up almost nothing while a folder part can still shrink; once they are all at
 * their minimum, its weight of 1 is the only one left and it takes the rest.
 */
const shrinkOf = (i: number): number => 10_000 * (parts.value.folders.length - i);
</script>

<template>
  <span class="flex min-w-0 items-baseline whitespace-nowrap">
    <span v-for="(folder, i) in parts.folders" :key="i" class="min-w-[1.5ch] truncate" :style="{ flexShrink: shrinkOf(i) }">{{ folder }}</span>
    <span class="min-w-0 truncate">{{ parts.last }}</span>
  </span>
</template>
