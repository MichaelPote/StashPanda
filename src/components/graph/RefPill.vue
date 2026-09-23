<script setup lang="ts">
import { computed } from "vue";
import { TagIcon } from "@lucide/vue";
import type { RefLabel } from "@/lib/bindings";
import { cn } from "@/lib/utils";

const props = defineProps<{
  label: RefLabel;
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
      return label.isCurrent ? "pill-current font-semibold" : "pill-local";
    case "remoteBranch":
      return "pill-remote";
    case "tag":
      return "pill-tag font-medium";
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
        'pill-flat inline-flex max-w-[12rem] shrink-0 items-center gap-0.5 px-1.5 text-[0.7rem] leading-[1.3] whitespace-nowrap',
        classes,
      )
    "
    :title="title"
  >
    <TagIcon v-if="label.kind.type === 'tag'" class="size-2.5 shrink-0" />
    <span class="truncate">{{ text }}</span>
  </button>
</template>
