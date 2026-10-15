<script setup lang="ts">
import { computed, inject } from "vue";
import type { RefLabel } from "@/lib/bindings";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { GRAPH_CONTEXT } from "@/components/graph/context";
import RefContextMenu from "@/components/graph/RefContextMenu.vue";
import RefPill, { type PillShape } from "@/components/graph/RefPill.vue";
import { laneVars } from "@/lib/graph/paths";
import { requestCheckout } from "@/lib/repoActions";

const props = defineProps<{
  refs: RefLabel[];
  /** The commit these labels sit on; the branch menu needs it as the ref's target. */
  oid: string;
  /** The row's pill colour, used by the detached HEAD pill; each ref pill carries its own. */
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

/**
 * Double-clicking a branch's pill checks the branch out, as in the Branches panel: a local branch
 * directly, a remote branch through a local branch of the same name. The checked-out branch and
 * tags do nothing.
 */
function checkoutOnDoubleClick(label: RefLabel): void {
  if (!sessionKey || label.isCurrent) return;
  if (label.kind.type === "localBranch") void requestCheckout(sessionKey, { kind: "local", name: label.short });
  else if (label.kind.type === "remoteBranch") {
    const prefix = `${label.kind.remote}/`;
    const local = label.short.startsWith(prefix) ? label.short.slice(prefix.length) : label.short;
    void requestCheckout(sessionKey, { kind: "remote", remote: label.short, local });
  }
}

/** A local branch and its upstream on different commits get the two halves of a chevron. */
function shapeFor(label: RefLabel): PillShape {
  if (!ctx) return "flat";
  if (label.kind.type === "localBranch") {
    return !label.upstreamAtSameCommit && ctx.upstreamRemotes.value.has(label.fullName) ? "tail" : "flat";
  }
  if (label.kind.type === "remoteBranch") return ctx.trackedRemotes.value.has(label.fullName) ? "head" : "flat";
  return "flat";
}
</script>

<template>
  <span
    v-if="detachedHead"
    class="pill-flat pill-lane-solid inline-flex shrink-0 items-center px-1.5 text-sm leading-none font-normal"
    :style="laneVars(colour)"
    title="Detached HEAD"
  >
    <span class="pill-text">HEAD</span>
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
      <RefPill
        :label="label"
        :colour="label.colour"
        :upstream-remote="remoteFor(label)"
        :shape="shapeFor(label)"
        @dblclick.stop="checkoutOnDoubleClick(label)"
      />
    </RefContextMenu>
    <RefPill v-else :label="label" :colour="label.colour" :upstream-remote="remoteFor(label)" :shape="shapeFor(label)" />
  </template>
  <Popover v-if="hidden.length > 0">
    <PopoverTrigger as-child>
      <button
        type="button"
        class="pill-flat pill-remote inline-flex shrink-0 items-center px-1.5 text-sm leading-none font-normal hover:text-foreground"
        :title="`${hidden.length} more`"
      >
        <span class="pill-text">+{{ hidden.length }}</span>
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
            <RefPill
              :label="label"
              :colour="label.colour"
              :upstream-remote="remoteFor(label)"
              :shape="shapeFor(label)"
              @dblclick.stop="checkoutOnDoubleClick(label)"
            />
          </RefContextMenu>
          <RefPill v-else :label="label" :colour="label.colour" :upstream-remote="remoteFor(label)" :shape="shapeFor(label)" />
        </template>
      </div>
    </PopoverContent>
  </Popover>
</template>
