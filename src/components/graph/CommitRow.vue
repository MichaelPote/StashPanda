<script setup lang="ts">
import { computed, inject } from "vue";
import type { GraphRow } from "@/lib/bindings";
import { GRAPH_CONTEXT } from "@/components/graph/context";
import RefLabels from "@/components/graph/RefLabels.vue";
import { formatAbsoluteTime, formatRelativeTime, hashString, initials, isoTime, shortOid } from "@/lib/format";
import { laneColour, PALETTE_SIZE } from "@/lib/graph/paths";
import { cn } from "@/lib/utils";

const props = defineProps<{
  row: GraphRow;
  selected: boolean;
}>();

const ctx = inject(GRAPH_CONTEXT, null);

const avatarColour = computed(() => laneColour(hashString(props.row.authorEmail.toLowerCase()) % PALETTE_SIZE));
const avatarText = computed(() => initials(props.row.authorName));
const detachedHead = computed(() => props.row.isHead && !props.row.refs.some((r) => r.isCurrent));

const dateText = computed(() =>
  ctx?.dateFormat.value === "absolute"
    ? formatAbsoluteTime(props.row.authorTime)
    : formatRelativeTime(props.row.authorTime),
);
</script>

<template>
  <div
    :class="cn('graph-grid graph-row h-(--row-h) text-base', selected ? 'row-selected' : row.isHead ? 'bg-row-head hover:bg-row-head-hover' : 'hover:bg-row-hover')"
    :data-index="row.index"
    role="row"
    :aria-selected="selected"
  >
    <div />
    <!-- Pills always show their whole name; the subject truncates. A column narrower than the
         pills clips them at its edge rather than letting them cover the next column. -->
    <div class="flex min-w-0 items-center gap-1 overflow-hidden pr-2 leading-(--row-h)">
      <RefLabels v-if="row.refs.length > 0 || detachedHead" :refs="row.refs" :oid="row.oid" :colour="row.refColour" :detached-head="detachedHead" />
      <span class="min-w-0 truncate" :title="row.subject">{{ row.subject }}</span>
    </div>
    <!-- A size container, so the name can hide once the column is too narrow for even one letter
         and an ellipsis; the column then shows only the avatars. -->
    <div class="@container flex min-w-0 items-center gap-1.5 px-1.5">
      <span
        class="inline-flex size-4 shrink-0 items-center justify-center rounded-full text-[0.55rem] leading-none font-semibold text-black/80"
        :style="{ backgroundColor: avatarColour }"
        aria-hidden="true"
      >
        <span class="text-cap-box">{{ avatarText }}</span>
      </span>
      <span class="truncate @max-[40px]:hidden" :title="`${row.authorName} <${row.authorEmail}>`">{{ row.authorName }}</span>
    </div>
    <div class="truncate px-1.5 leading-(--row-h) text-muted-foreground tabular-nums" :title="isoTime(row.authorTime)">
      {{ dateText }}
    </div>
    <div class="truncate px-1.5 font-mono text-xs leading-(--row-h) text-muted-foreground select-text" :title="row.oid">
      {{ shortOid(row.oid) }}
    </div>
  </div>
</template>
