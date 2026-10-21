<script setup lang="ts">
import { computed } from "vue";
import { ChevronRightIcon, TagIcon } from "@lucide/vue";
import { Tip } from "@/components/ui/tooltip";
import type { RefLabel } from "@/lib/bindings";
import { laneVars } from "@/lib/graph/paths";
import { cn } from "@/lib/utils";
import { useSettingsStore } from "@/stores/settings";

/** `tail` has a chevron cut into its left side; `head` has the matching point on its right. */
export type PillShape = "flat" | "tail" | "head";

// The root is the tooltip, which would drop the attributes and listeners a parent passes (the
// context menu's trigger, a double-click), so they go on the button itself.
defineOptions({ inheritAttrs: false });

const props = defineProps<{
  label: RefLabel;
  /** Palette index for the pill: the lane's colour, or its own on a branch tip below the lane's top one. */
  colour: number;
  /** Remote name of the branch's upstream, for the collapsed `origin | dev` form. */
  upstreamRemote?: string | null;
  shape?: PillShape;
}>();

/** The remote shown before the chevron seam when the upstream is on the same commit. */
const parityRemote = computed(() =>
  props.label.kind.type === "localBranch" && props.label.upstreamAtSameCommit ? (props.upstreamRemote ?? null) : null,
);

/** The checked-out branch: its pill gets a HEAD ring before the name and a sheen over its fill. */
const isCurrent = computed(() => props.label.kind.type === "localBranch" && props.label.isCurrent);

/** A remote branch's remote, shown before a chevron and the branch name (`origin > dev`). */
const remoteName = computed(() => (props.label.kind.type === "remoteBranch" ? props.label.kind.remote : null));

const text = computed(() => {
  const { label } = props;
  switch (label.kind.type) {
    case "localBranch":
      return label.short;
    case "remoteBranch": {
      const prefix = `${label.kind.remote}/`;
      const rest = label.short.startsWith(prefix) ? label.short.slice(prefix.length) : label.short;
      return rest;
    }
    case "tag":
      return label.short;
  }
});

const shapeClass = computed(() => {
  switch (props.shape ?? "flat") {
    case "flat":
      return "pill-flat px-1.5";
    case "tail":
      return "pill-tail";
    case "head":
      return "pill-head";
  }
});

const settings = useSettingsStore();

/**
 * Local branches are solid and remote branches outlined, or the other way round when
 * `appearance.solidRemotePills` is on. Tags are always solid.
 */
const swapped = computed(() => settings.settings?.appearance?.solidRemotePills ?? false);
const localClass = computed(() => (swapped.value ? "pill-lane" : "pill-lane-solid"));
const remoteClass = computed(() => (swapped.value ? "pill-lane-solid" : "pill-lane"));

const colourClass = computed(() => {
  switch (props.label.kind.type) {
    case "localBranch":
      return localClass.value;
    case "remoteBranch":
      return remoteClass.value;
    case "tag":
      return "pill-lane-solid";
  }
});

const title = computed(() => {
  const { label } = props;
  const parts = [label.fullName];
  if (label.ahead) parts.push(`↑${label.ahead}`);
  if (label.behind) parts.push(`↓${label.behind}`);
  return parts.join(" ") + (isCurrent.value ? " (checked out)" : "");
});
</script>

<template>
  <!-- A local branch on the same commit as its upstream: two pills that interlock at a chevron,
       the remote styled like a remote branch and the branch like a local one. The branch
       pill moves left by the chevron's depth plus 1px, so the remote pill's point sits in its
       notch with the edges overlapping instead of leaving a hairline between them. One button
       holds both, so the context menu and tooltip cover the pair. -->
  <Tip v-if="parityRemote" :content="title">
    <button
      v-bind="$attrs"
      type="button"
      class="inline-flex shrink-0 items-stretch text-sm leading-none font-normal whitespace-nowrap"
      :style="laneVars(colour)"
    >
      <span :class="cn('pill-head inline-flex items-center', remoteClass)">
        <span class="pill-text">{{ parityRemote }}</span>
      </span>
      <span
        :class="cn('pill-tail inline-flex items-center gap-1', localClass, isCurrent && 'pill-current')"
        :style="{ marginLeft: 'calc(-1 * var(--pill-notch) - 1px)' }"
      >
        <!-- The HEAD ring: not a Lucide icon, so the global svg.lucide stroke rule leaves it alone. -->
        <svg v-if="isCurrent" width="9" height="9" viewBox="0 0 9 9" class="shrink-0" aria-hidden="true">
          <circle cx="4.5" cy="4.5" r="3.4" fill="none" stroke="currentColor" stroke-width="1.5" />
          <circle cx="4.5" cy="4.5" r="1.4" fill="currentColor" />
        </svg>
        <span class="pill-text">{{ text }}</span>
      </span>
    </button>
  </Tip>
  <Tip v-else :content="title">
    <button
      v-bind="$attrs"
      type="button"
      :class="
        cn(
          'inline-flex shrink-0 items-center gap-1 text-sm leading-none font-normal whitespace-nowrap',
          shapeClass,
          colourClass,
          isCurrent && 'pill-current',
        )
      "
      :style="laneVars(colour)"
    >
      <TagIcon v-if="label.kind.type === 'tag'" class="size-3 shrink-0" />
      <template v-if="remoteName">
        <span class="pill-text">{{ remoteName }}</span>
        <ChevronRightIcon class="-mx-0.5 size-3 shrink-0 [stroke-width:1.5px]" aria-hidden="true" />
      </template>
      <!-- The HEAD ring on the checked-out branch: not a Lucide icon, so the global svg.lucide stroke
           rule leaves it alone, and currentColor gives it the pill's text colour. -->
      <svg v-if="isCurrent" width="9" height="9" viewBox="0 0 9 9" class="shrink-0" aria-hidden="true">
        <circle cx="4.5" cy="4.5" r="3.4" fill="none" stroke="currentColor" stroke-width="1.5" />
        <circle cx="4.5" cy="4.5" r="1.4" fill="currentColor" />
      </svg>
      <span class="pill-text">{{ text }}</span>
    </button>
  </Tip>
</template>
