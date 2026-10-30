<script setup lang="ts">
import { computed } from "vue";
import { ContextMenu, ContextMenuContent, ContextMenuItem, ContextMenuSeparator, ContextMenuTrigger } from "@/components/ui/context-menu";
import ActionMenuItem from "@/components/graph/ActionMenuItem.vue";
import { shortSha } from "@/lib/refNames";
import { createBranchHere } from "@/lib/repoActions";
import { useSessionsStore } from "@/stores/sessions";

/**
 * The right-click menu of a heading in the Branches panel: a section, a remote or a branch folder.
 * "Add Branch…" opens the Create Branch dialog at the checked-out commit; from a folder, the new
 * branch's name starts with the folder's path (`feature/`). With `refs` or `stashes`, Show All and
 * Hide All switch everything under the heading on or off in the graph at once. The default slot
 * is the trigger; the `items` slot adds a heading's own items at the end.
 */
const props = defineProps<{
  sessionKey: string;
  /** The start of the new branch's name, for a folder's heading. */
  prefix?: string;
  /** Full names of the refs under the heading, at any depth, for Show All and Hide All. */
  refs?: readonly string[];
  /** Commit IDs of the stashes, for the Stashes heading's Show All and Hide All. */
  stashes?: readonly string[];
  /** What the refs are, for the items' names. */
  noun?: "Branches" | "Tags" | "Stashes";
}>();

const sessions = useSessionsStore();
const session = computed(() => sessions.sessions.get(props.sessionKey) ?? null);

const noun = computed(() => props.noun ?? "Branches");
const singular = { Branches: "branch", Tags: "tag", Stashes: "stash" } as const;
/** What Show All and Hide All switch: refs, hidden when listed, or stashes, hidden unless listed. */
const items = computed(() => props.refs ?? props.stashes ?? null);
const hiddenCount = computed(() => {
  const s = session.value;
  if (!s) return 0;
  if (props.refs) return props.refs.filter((r) => s.hiddenRefs.has(r)).length;
  return (props.stashes ?? []).filter((oid) => !s.shownStashes.has(oid)).length;
});
const showAllReason = computed(() => {
  if (!items.value?.length) return `There are no ${noun.value.toLowerCase()} here.`;
  return hiddenCount.value === 0 ? `No ${noun.value.toLowerCase()} here are hidden.` : null;
});
const hideAllReason = computed(() => {
  if (!items.value?.length) return `There are no ${noun.value.toLowerCase()} here.`;
  return hiddenCount.value === items.value.length ? `Every ${singular[noun.value]} here is already hidden.` : null;
});

function addBranch(): void {
  const s = session.value;
  if (!s) return;
  const head = s.status?.headOid ?? null;
  const label = s.status?.branch ?? (head ? shortSha(head) : "HEAD");
  void createBranchHere(props.sessionKey, "HEAD", label, props.prefix);
}

function setAllVisible(visible: boolean): void {
  if (props.refs) void sessions.setRefsVisible(props.sessionKey, props.refs, visible);
  else if (props.stashes) sessions.setStashesShown(props.sessionKey, props.stashes, visible);
}
</script>

<template>
  <ContextMenu>
    <ContextMenuTrigger as-child>
      <slot />
    </ContextMenuTrigger>
    <ContextMenuContent class="min-w-44">
      <ContextMenuItem class="text-base" data-add-branch @select="addBranch">Add Branch…</ContextMenuItem>
      <template v-if="items">
        <ContextMenuSeparator />
        <ActionMenuItem :reason="showAllReason" data-show-all-refs @select="setAllVisible(true)">Show All {{ noun }}</ActionMenuItem>
        <ActionMenuItem :reason="hideAllReason" data-hide-all-refs @select="setAllVisible(false)">Hide All {{ noun }}</ActionMenuItem>
      </template>
      <slot name="items" />
    </ContextMenuContent>
  </ContextMenu>
</template>
