<script setup lang="ts">
import { computed, inject } from "vue";
import type { GraphRow, RefLabel, StashEntry } from "@/lib/bindings";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { Tip } from "@/components/ui/tooltip";
import { GRAPH_CONTEXT } from "@/components/graph/context";
import RefContextMenu from "@/components/graph/RefContextMenu.vue";
import RefPill, { type PillShape } from "@/components/graph/RefPill.vue";
import RemoteTagContextMenu from "@/components/graph/RemoteTagContextMenu.vue";
import StashContextMenu from "@/components/graph/StashContextMenu.vue";
import StashPill from "@/components/graph/StashPill.vue";
import { laneVars } from "@/lib/graph/paths";
import { requestCheckout } from "@/lib/repoActions";
import { localOnlyNote, remoteOnlyNote, type RemoteOnlyTag } from "@/lib/tagSync";

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

/**
 * One pill: a ref here, or a tag a remote has on this commit that is not here on it (`remote`).
 * `sync` and `note` mark a tag that is not on every remote compared.
 */
interface RefItem {
  key: string;
  label: RefLabel;
  sync: "localOnly" | "remoteOnly" | null;
  note: string;
  remote: RemoteOnlyTag | null;
  stash: null;
}

/** A stash made on this commit and switched on in the Branches panel. */
interface StashItem {
  key: string;
  stash: StashEntry;
}

type Item = RefItem | StashItem;

const items = computed<Item[]>(() => {
  const view = ctx?.tagSync.value ?? null;
  const out: Item[] = props.refs.map((label) => {
    const sync = label.kind.type === "tag" ? view?.local.get(label.short) : undefined;
    return sync?.state === "localOnly"
      ? { key: label.fullName, label, sync: "localOnly", note: localOnlyNote(sync), remote: null, stash: null }
      : { key: label.fullName, label, sync: null, note: "", remote: null, stash: null };
  });
  for (const tag of view?.remoteOnly.get(props.oid) ?? []) {
    out.push({
      key: `remote:refs/tags/${tag.name}`,
      label: {
        // Not a ref here: the prefix keeps its key apart from a local tag of the same name.
        fullName: `remote:refs/tags/${tag.name}`,
        short: tag.name,
        kind: { type: "tag", annotated: false },
        isCurrent: false,
        colour: props.colour,
        upstreamAtSameCommit: false,
        ahead: null,
        behind: null,
      },
      sync: "remoteOnly",
      note: remoteOnlyNote(tag, view?.local.has(tag.name) ?? false),
      remote: tag,
      stash: null,
    });
  }
  for (const stash of ctx?.stashesShown.value.get(props.oid) ?? []) out.push({ key: `stash:${stash.oid}`, stash });
  return out;
});

/** A stash's pill takes the lane colour of the commit it was made on. */
const stashColour = computed(() => props.row?.colour ?? props.colour);

const visible = computed(() => items.value.slice(0, MAX_VISIBLE));
const hidden = computed(() => items.value.slice(MAX_VISIBLE));

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
  <template v-for="item in visible" :key="item.key">
    <template v-if="item.stash">
      <StashContextMenu v-if="sessionKey" :session-key="sessionKey" :stash="item.stash">
        <StashPill :stash="item.stash" :colour="stashColour" />
      </StashContextMenu>
      <StashPill v-else :stash="item.stash" :colour="stashColour" />
    </template>
    <RemoteTagContextMenu v-else-if="sessionKey && item.remote" :session-key="sessionKey" :name="item.remote.name" :remotes="item.remote.remotes">
      <RefPill :label="item.label" :colour="item.label.colour" :oid="oid" sync="remoteOnly" :sync-note="item.note" />
    </RemoteTagContextMenu>
    <RefContextMenu
      v-else-if="sessionKey"
      :session-key="sessionKey"
      :full-name="item.label.fullName"
      :short="item.label.short"
      :kind="item.label.kind"
      :target="oid"
      :is-current="item.label.isCurrent"
      :row="row"
    >
      <RefPill
        :label="item.label"
        :colour="item.label.colour" :oid="oid"
        :upstream-remote="remoteFor(item.label)"
        :shape="shapeFor(item.label)"
        :sync="item.sync"
        :sync-note="item.note"
        @dblclick.stop="checkoutOnDoubleClick(item.label)"
      />
    </RefContextMenu>
    <RefPill v-else :label="item.label" :colour="item.label.colour" :oid="oid" :upstream-remote="remoteFor(item.label)" :shape="shapeFor(item.label)" :sync="item.sync" :sync-note="item.note" />
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
        <template v-for="item in hidden" :key="item.key">
          <template v-if="item.stash">
            <StashContextMenu v-if="sessionKey" :session-key="sessionKey" :stash="item.stash">
              <StashPill :stash="item.stash" :colour="stashColour" />
            </StashContextMenu>
            <StashPill v-else :stash="item.stash" :colour="stashColour" />
          </template>
          <RemoteTagContextMenu v-else-if="sessionKey && item.remote" :session-key="sessionKey" :name="item.remote.name" :remotes="item.remote.remotes">
            <RefPill :label="item.label" :colour="item.label.colour" :oid="oid" sync="remoteOnly" :sync-note="item.note" />
          </RemoteTagContextMenu>
          <RefContextMenu
            v-else-if="sessionKey"
            :session-key="sessionKey"
            :full-name="item.label.fullName"
            :short="item.label.short"
            :kind="item.label.kind"
            :target="oid"
            :is-current="item.label.isCurrent"
            :row="row"
          >
            <RefPill
              :label="item.label"
              :colour="item.label.colour" :oid="oid"
              :upstream-remote="remoteFor(item.label)"
              :shape="shapeFor(item.label)"
              :sync="item.sync"
              :sync-note="item.note"
              @dblclick.stop="checkoutOnDoubleClick(item.label)"
            />
          </RefContextMenu>
          <RefPill v-else :label="item.label" :colour="item.label.colour" :oid="oid" :upstream-remote="remoteFor(item.label)" :shape="shapeFor(item.label)" :sync="item.sync" :sync-note="item.note" />
        </template>
      </div>
    </PopoverContent>
  </Popover>
</template>
