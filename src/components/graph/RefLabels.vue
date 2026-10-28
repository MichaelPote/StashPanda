<script setup lang="ts">
import { computed, inject } from "vue";
import type { GraphRow, RefLabel } from "@/lib/bindings";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { Tip } from "@/components/ui/tooltip";
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
  /** The row the labels sit on, which gives the branch menu the commit row's own items. */
  row?: GraphRow | null;
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
  <Tip v-if="detachedHead" content="Detached HEAD">
    <span
      class="pill-flat pill-lane-solid inline-flex shrink-0 items-center px-1.5 text-sm leading-none font-normal"
      :style="laneVars(colour)"
    >
      <span class="pill-text">HEAD</span>
    </span>
  </Tip>
  <template v-for="label in visible" :key="label.fullName">
    <RefContextMenu
      v-if="sessionKey"
      :session-key="sessionKey"
      :full-name="label.fullName"
      :short="label.short"
      :kind="label.kind"
      :target="oid"
      :is-current="label.isCurrent"
      :row="row"
    >
      <RefPill
        :label="label"
        :colour="label.colour" :oid="oid"
        :upstream-remote="remoteFor(label)"
        :shape="shapeFor(label)"
        @dblclick.stop="checkoutOnDoubleClick(label)"
      />
    </RefContextMenu>
    <RefPill v-else :label="label" :colour="label.colour" :oid="oid" :upstream-remote="remoteFor(label)" :shape="shapeFor(label)" />
  </template>
  <Popover v-if="hidden.length > 0">
    <PopoverTrigger as-child>
      <button
        type="button"
        class="pill-flat pill-remote inline-flex shrink-0 items-center px-1.5 text-sm leading-none font-normal hover:text-foreground"
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
            :row="row"
          >
            <RefPill
              :label="label"
              :colour="label.colour" :oid="oid"
              :upstream-remote="remoteFor(label)"
              :shape="shapeFor(label)"
              @dblclick.stop="checkoutOnDoubleClick(label)"
            />
          </RefContextMenu>
          <RefPill v-else :label="label" :colour="label.colour" :oid="oid" :upstream-remote="remoteFor(label)" :shape="shapeFor(label)" />
        </template>
      </div>
    </PopoverContent>
  </Popover>
</template>
