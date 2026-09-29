<script setup lang="ts">
import { computed, inject } from "vue";
import type { RefLabel } from "@/lib/bindings";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { GRAPH_CONTEXT } from "@/components/graph/context";
import RefContextMenu from "@/components/graph/RefContextMenu.vue";
import RefPill from "@/components/graph/RefPill.vue";
import { laneVars } from "@/lib/graph/paths";

const props = defineProps<{
  refs: RefLabel[];
  /** The commit these labels sit on; the branch menu needs it as the ref's target. */
  oid: string;
  /** Lane palette index of the commit's lane; the pills take this lane's colour. */
  colour: number;
  /** True on HEAD's row when no local branch is current: shows a detached HEAD pill. */
  detachedHead: boolean;
}>();

const MAX_VISIBLE = 3;

const ctx = inject(GRAPH_CONTEXT, null);
/** Without a graph context (no session) the pills are plain and have no menu. */
const sessionKey = ctx?.sessionKey ?? null;

const visible = computed(() => props.refs.slice(0, MAX_VISIBLE));
const hidden = computed(() => props.refs.slice(MAX_VISIBLE));

function remoteFor(label: RefLabel): string | null {
  return ctx?.upstreamRemotes.value.get(label.fullName) ?? null;
}
</script>

<template>
  <span
    v-if="detachedHead"
    class="pill-flat pill-lane-solid inline-flex shrink-0 items-center px-[7px] text-xs leading-none font-semibold"
    :style="laneVars(colour)"
    title="Detached HEAD"
  >
    HEAD
  </span>
  <template v-for="label in visible" :key="label.fullName">
    <RefContextMenu
      v-if="sessionKey"
      :session-key="sessionKey"
      :full-name="label.fullName"
      :short="label.short"
      :kind="label.kind"
      :target="oid"
      :is-current="label.isCurrent"
    >
      <RefPill :label="label" :colour="colour" :upstream-remote="remoteFor(label)" />
    </RefContextMenu>
    <RefPill v-else :label="label" :colour="colour" :upstream-remote="remoteFor(label)" />
  </template>
  <Popover v-if="hidden.length > 0">
    <PopoverTrigger as-child>
      <button
        type="button"
        class="pill-flat pill-remote inline-flex shrink-0 items-center px-[7px] text-xs leading-none font-semibold hover:text-foreground"
        :title="`${hidden.length} more`"
      >
        +{{ hidden.length }}
      </button>
    </PopoverTrigger>
    <PopoverContent align="start" class="w-auto max-w-xs p-2">
      <div class="flex flex-wrap gap-1">
        <template v-for="label in hidden" :key="label.fullName">
          <RefContextMenu
            v-if="sessionKey"
            :session-key="sessionKey"
            :full-name="label.fullName"
            :short="label.short"
            :kind="label.kind"
            :target="oid"
            :is-current="label.isCurrent"
          >
            <RefPill :label="label" :colour="colour" :upstream-remote="remoteFor(label)" />
          </RefContextMenu>
          <RefPill v-else :label="label" :colour="colour" :upstream-remote="remoteFor(label)" />
        </template>
      </div>
    </PopoverContent>
  </Popover>
</template>
