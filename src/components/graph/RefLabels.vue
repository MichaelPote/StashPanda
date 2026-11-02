<script setup lang="ts">
import { computed, inject, onMounted, onUpdated, ref, watch } from "vue";
import type { GraphRow, RefLabel, StashEntry } from "@/lib/bindings";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { Tip } from "@/components/ui/tooltip";
import { GRAPH_CONTEXT } from "@/components/graph/context";
import RefContextMenu from "@/components/graph/RefContextMenu.vue";
import RefPill, { type PillShape } from "@/components/graph/RefPill.vue";
import RemoteTagContextMenu from "@/components/graph/RemoteTagContextMenu.vue";
import StashContextMenu from "@/components/graph/StashContextMenu.vue";
import StashPill from "@/components/graph/StashPill.vue";
import { fontsVersion } from "@/lib/graph/columnWidths";
import { laneVars, rootFontSizePx } from "@/lib/graph/paths";
import { pillsThatFit } from "@/lib/graph/pillFit";
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
  /** Pixels the pills may take; those that do not fit go into a "+N" pill. Without it all show. */
  room?: number;
}>();

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

/**
 * Every pill is drawn, and each one's width read from the row once it is: the ones that do not fit
 * stay there, unseen and out of the layout, so a wider column can show them again without drawing
 * them first. The hidden copy of the "+N" pill, counting every pill, gives the room to keep for it.
 */
const box = ref<HTMLElement | null>(null);
const moreProbe = ref<HTMLElement | null>(null);
/** Each item's pill width, in `items` order; the detached HEAD pill's; and the "+N" pill's. */
const widths = ref<number[]>([]);
const headWidth = ref(0);
const moreWidth = ref(0);

function measure(): void {
  const el = box.value;
  if (!el) return;
  const next = Array.from(el.querySelectorAll<HTMLElement>(":scope > [data-pill]"), (p) => p.getBoundingClientRect().width);
  if (next.length !== widths.value.length || next.some((w, i) => w !== widths.value[i])) widths.value = next;
  const head = el.querySelector<HTMLElement>(":scope > [data-head]")?.getBoundingClientRect().width ?? 0;
  if (head !== headWidth.value) headWidth.value = head;
  const more = moreProbe.value?.getBoundingClientRect().width ?? 0;
  if (more !== moreWidth.value) moreWidth.value = more;
}
// After each draw, before the browser paints it: a change in the widths draws once more.
onMounted(measure);
onUpdated(measure);
// The web fonts change the widths without drawing the row again.
watch(fontsVersion, measure);

/** How many pills show; until the widths are read, all of them. A detached HEAD pill comes first. */
const shownCount = computed(() => {
  const n = items.value.length;
  if (props.room === undefined || widths.value.length !== n) return n;
  const gap = 0.25 * rootFontSizePx();
  const head = props.detachedHead ? headWidth.value + gap : 0;
  return pillsThatFit(widths.value, props.room - head, gap, moreWidth.value, props.detachedHead ? 0 : 1);
});
const hidden = computed(() => items.value.slice(shownCount.value));

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
  <span ref="box" class="relative flex shrink-0 items-center gap-1">
    <span v-if="detachedHead" data-head class="inline-flex shrink-0">
      <Tip content="Detached HEAD">
        <span
          class="pill-flat pill-lane-solid inline-flex shrink-0 items-center px-1.5 text-sm leading-none font-normal"
          :style="laneVars(colour)"
        >
          <span class="pill-text">HEAD</span>
        </span>
      </Tip>
    </span>
    <span
      v-for="(item, i) in items"
      :key="item.key"
      data-pill
      :class="i < shownCount ? 'inline-flex shrink-0' : 'invisible absolute top-0 left-0 inline-flex w-max'"
    >
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
    </span>
    <span
      ref="moreProbe"
      aria-hidden="true"
      class="pill-flat pill-remote invisible absolute top-0 left-0 inline-flex w-max items-center px-1.5 text-sm leading-none font-normal"
    >
      <span class="pill-text">+{{ items.length }}</span>
    </span>
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
  </span>
</template>
