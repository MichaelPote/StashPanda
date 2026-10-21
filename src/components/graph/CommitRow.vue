<script setup lang="ts">
import { computed, inject } from "vue";
import type { GraphRow } from "@/lib/bindings";
import { GRAPH_CONTEXT } from "@/components/graph/context";
import RefLabels from "@/components/graph/RefLabels.vue";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { useCommitColumns } from "@/composables/useCommitColumns";
import { formatAbsoluteTime, formatRelativeTime, hashString, initials, isoTime, shortOid } from "@/lib/format";
import { matchSegments, oidSegments } from "@/lib/graph/find";
import { laneColour, PALETTE_SIZE } from "@/lib/graph/paths";
import { cn } from "@/lib/utils";
import { gravatarMissing, gravatarUrl } from "@/lib/avatars";
import { useSettingsStore } from "@/stores/settings";

const props = defineProps<{
  row: GraphRow;
  selected: boolean;
  /** Where the message starts, in pixels, when the lanes are drawn inside its column; null when they have their own. */
  indent?: number | null;
}>();

const ctx = inject(GRAPH_CONTEXT, null);

const avatarColour = computed(() => laneColour(hashString(props.row.authorEmail.toLowerCase()) % PALETTE_SIZE));
const avatarText = computed(() => initials(props.row.authorName));
const settings = useSettingsStore();
/** The author's Gravatar picture when the setting asks for it and Gravatar has one. */
const pictureUrl = computed(() => (settings.settings?.appearance?.avatarSource === "gravatar" ? gravatarUrl(props.row.authorEmail) : null));
const detachedHead = computed(() => props.row.isHead && !props.row.refs.some((r) => r.isCurrent));
/** `appearance.authorAvatarOnly`: the picture alone; the tooltip still has the name and email address. */
const avatarOnly = computed(() => settings.settings?.appearance?.authorAvatarOnly ?? false);
/** Hidden columns keep an empty cell, at no width, so the grid's columns stay in place. */
const { shown } = useCommitColumns();

// The commit search's matches, cut out of what the row shows so each can have a pill.
const findText = computed(() => ctx?.findText.value ?? "");
const subjectParts = computed(() => matchSegments(props.row.subject, findText.value));
const authorParts = computed(() => matchSegments(props.row.authorName, findText.value));
const idParts = computed(() => oidSegments(shortOid(props.row.oid), props.row.oid, findText.value));

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
    <div class="flex min-w-0 items-center gap-1 overflow-hidden pr-2 leading-(--row-h)" :style="indent != null ? { paddingLeft: `${indent}px` } : undefined">
      <RefLabels v-if="row.refs.length > 0 || detachedHead" :refs="row.refs" :oid="row.oid" :colour="row.refColour" :detached-head="detachedHead" />
      <span class="min-w-0 truncate" :title="row.subject"><template v-for="(part, i) in subjectParts" :key="i"><mark v-if="part.match" class="find-match">{{ part.text }}</mark><template v-else>{{ part.text }}</template></template></span>
    </div>
    <!-- A size container, so the name can hide once the column is too narrow for even one letter
         and an ellipsis; the column then shows only the avatars. -->
    <div v-if="shown.author" class="@container flex min-w-0 items-center gap-1.5 px-1.5">
      <!-- One tooltip for the picture and the name together: the name, then the email address. -->
      <Tooltip>
        <TooltipTrigger as-child>
          <span class="flex min-w-0 items-center gap-1.5">
            <img
              v-if="pictureUrl"
              :src="pictureUrl"
              class="size-4 shrink-0 rounded-full"
              :alt="avatarOnly ? row.authorName : ''"
              :aria-hidden="!avatarOnly"
              @error="gravatarMissing(row.authorEmail)"
            />
            <span
              v-else
              class="inline-flex size-4 shrink-0 items-center justify-center rounded-full text-[0.55rem] leading-none font-semibold text-black/80"
              :style="{ backgroundColor: avatarColour }"
              :aria-hidden="!avatarOnly"
              :aria-label="avatarOnly ? row.authorName : undefined"
            >
              <span class="text-cap-box">{{ avatarText }}</span>
            </span>
            <span v-if="!avatarOnly" class="truncate @max-[40px]:hidden"><template v-for="(part, i) in authorParts" :key="i"><mark v-if="part.match" class="find-match">{{ part.text }}</mark><template v-else>{{ part.text }}</template></template></span>
          </span>
        </TooltipTrigger>
        <TooltipContent side="right">
          <!-- A column: the tooltip lays its content out in a row. -->
          <div class="flex flex-col">
            <span class="font-medium">{{ row.authorName }}</span>
            <!-- An address has no spaces to wrap at, so a long one would run past the tooltip's width. -->
            <span class="[overflow-wrap:anywhere]">{{ row.authorEmail }}</span>
          </div>
        </TooltipContent>
      </Tooltip>
    </div>
    <div v-else />
    <div v-if="shown.date" class="truncate px-1.5 leading-(--row-h) text-muted-foreground tabular-nums" :title="isoTime(row.authorTime)">
      {{ dateText }}
    </div>
    <div v-else />
    <div v-if="shown.id" class="truncate px-1.5 font-mono text-xs leading-(--row-h) text-muted-foreground select-text" :title="row.oid">
      <template v-for="(part, i) in idParts" :key="i"><mark v-if="part.match" class="find-match">{{ part.text }}</mark><template v-else>{{ part.text }}</template></template>
    </div>
    <div v-else />
  </div>
</template>
