<script setup lang="ts">
import { computed } from "vue";
import { TagIcon } from "@lucide/vue";
import type { RefLabel } from "@/lib/bindings";
import { laneVars } from "@/lib/graph/paths";
import { cn } from "@/lib/utils";

const props = defineProps<{
  label: RefLabel;
  /** Lane palette index of the commit the ref points at; the pill takes this lane's colour. */
  colour: number;
  /** Remote name of the branch's upstream, for the collapsed `dev ▸ origin` form. */
  upstreamRemote?: string | null;
}>();

const text = computed(() => {
  const { label } = props;
  switch (label.kind.type) {
    case "localBranch":
      return label.upstreamAtSameCommit ? `${label.short} ▸ ${props.upstreamRemote ?? ""}`.trimEnd() : label.short;
    case "remoteBranch": {
      const prefix = `${label.kind.remote}/`;
      const rest = label.short.startsWith(prefix) ? label.short.slice(prefix.length) : label.short;
      return `${label.kind.remote} ▸ ${rest}`;
    }
    case "tag":
      return label.short;
  }
});

const classes = computed(() => {
  const { label } = props;
  switch (label.kind.type) {
    case "localBranch":
      return label.isCurrent ? "pill-lane-solid" : "pill-lane";
    case "remoteBranch":
      return "pill-lane";
    case "tag":
      return "pill-lane-solid";
  }
});

const title = computed(() => {
  const { label } = props;
  const parts = [label.fullName];
  if (label.ahead) parts.push(`↑${label.ahead}`);
  if (label.behind) parts.push(`↓${label.behind}`);
  return parts.join(" ");
});
</script>

<template>
  <button
    type="button"
    :class="
      cn(
        'pill-flat inline-flex shrink-0 items-center gap-1 px-[7px] text-xs leading-none font-semibold whitespace-nowrap',
        classes,
      )
    "
    :style="laneVars(colour)"
    :title="title"
  >
    <TagIcon v-if="label.kind.type === 'tag'" class="size-2.5 shrink-0" />
    <span>{{ text }}</span>
  </button>
</template>
