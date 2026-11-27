<script setup lang="ts">
import { computed } from "vue";
import type { GraphRow, RefLabel } from "@/lib/bindings";
import { ContextMenuSub, ContextMenuSubContent, ContextMenuSubTrigger } from "@/components/ui/context-menu";
import ActionMenuItem from "@/components/graph/ActionMenuItem.vue";
import LaneBranchName from "@/components/graph/LaneBranchName.vue";
import { shortSha } from "@/lib/refNames";
import { createBranchHere, currentBranch, operationReason, isDetached, mergeInto, requestCheckout, revisionOf } from "@/lib/repoActions";
import type { RepoSession } from "@/stores/sessions";

/**
 * The first items of a commit's menu: Checkout, Create Branch Here and Merge, for the commit on
 * `row`. The commit row's own menu starts with them, and so does the part of the menu of a tag
 * that only a remote has which is about the tag's commit. A branch's or a local tag's menu has
 * its own versions, which name the branch or tag (RefContextMenu.vue).
 */
const props = defineProps<{
  row: GraphRow;
  session: RepoSession;
}>();

const sha = computed(() => shortSha(props.row.oid));
const localBranches = computed(() => props.row.refs.filter((r) => r.kind.type === "localBranch"));
/** The one local branch on this row, when there is exactly one (a single item instead of a submenu). */
const singleBranch = computed<RefLabel | null>(() => (localBranches.value.length === 1 ? (localBranches.value[0] ?? null) : null));

const branch = computed(() => currentBranch(props.session));
/** The checked-out branch's full name, for its lane colour. */
const branchFullName = computed(() => (branch.value ? `refs/heads/${branch.value}` : ""));

// A dirty working tree does not block a checkout: requestCheckout asks first.
function checkoutReason(label: RefLabel): string | null {
  return label.isCurrent ? `${label.short} is already checked out.` : null;
}

/** Merge by branch name when the row has one, else by oid. */
const mergeSource = computed(() => {
  const first = localBranches.value[0];
  return first ? revisionOf(first, props.session.refs?.refs ?? []) : props.row.oid;
});
const mergeLabel = computed(() => localBranches.value[0]?.short ?? sha.value);
/** The branch merged, for its lane colour; null when merging by oid. */
const mergeFullName = computed(() => localBranches.value[0]?.fullName ?? null);
const mergeReason = computed(() => {
  if (props.row.isHead) return "This commit is HEAD already.";
  if (isDetached(props.session)) return "HEAD is detached. Check out a branch to merge into first.";
  // Uncommitted changes do not block it: mergeInto asks, then stashes them around the merge.
  return operationReason(props.session);
});
</script>

<template>
  <ActionMenuItem v-if="localBranches.length === 0" :reason="null" @select="requestCheckout(session.key, { kind: 'revision', revision: row.oid, label: sha })">
    Checkout {{ sha }}
  </ActionMenuItem>
  <ActionMenuItem v-else-if="singleBranch" :reason="checkoutReason(singleBranch)" @select="requestCheckout(session.key, { kind: 'local', name: singleBranch.short })">
    <!-- One <span>, because the item is a flex row whose gap would replace the spaces between words. -->
    <span>Checkout <LaneBranchName :session="session" :full-name="singleBranch.fullName" :name="singleBranch.short" /></span>
  </ActionMenuItem>
  <ContextMenuSub v-else>
    <ContextMenuSubTrigger class="text-base">Checkout</ContextMenuSubTrigger>
    <ContextMenuSubContent>
      <ActionMenuItem v-for="b in localBranches" :key="b.fullName" :reason="checkoutReason(b)" @select="requestCheckout(session.key, { kind: 'local', name: b.short })">
        <LaneBranchName :session="session" :full-name="b.fullName" :name="b.short" />
      </ActionMenuItem>
    </ContextMenuSubContent>
  </ContextMenuSub>

  <ActionMenuItem @select="createBranchHere(session.key, row.oid, sha)">Create Branch Here…</ActionMenuItem>

  <ActionMenuItem :reason="mergeReason" @select="mergeInto(session.key, mergeSource, mergeLabel)">
    <span>
      Merge
      <LaneBranchName v-if="mergeFullName" :session="session" :full-name="mergeFullName" :name="mergeLabel" />
      <template v-else>{{ mergeLabel }}</template>
      into
      <LaneBranchName v-if="branch" :session="session" :full-name="branchFullName" :name="branch" />
      <template v-else>current branch</template>
    </span>
  </ActionMenuItem>
</template>
