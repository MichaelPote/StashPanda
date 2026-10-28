<script setup lang="ts">
import { computed } from "vue";
import { laneColour } from "@/lib/graph/paths";
import type { RepoSession } from "@/stores/sessions";

/**
 * A branch name in a menu item, inked in the colour of the branch's lane in the commit graph.
 * Plain text when the branch has no lane: hidden, not in the graph, or not yet created.
 */
const props = defineProps<{
  session: RepoSession | null;
  /** `refs/heads/dev` or `refs/remotes/origin/dev` */
  fullName: string;
  name: string;
}>();

const colour = computed(() => props.session?.graph.meta?.refColours?.[props.fullName] ?? null);
</script>

<template>
  <span :style="colour !== null ? { color: laneColour(colour) } : undefined">{{ name }}</span>
</template>
