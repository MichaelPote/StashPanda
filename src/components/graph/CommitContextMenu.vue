<script setup lang="ts">
import { computed } from "vue";
import type { GraphRow, RefLabel } from "@/lib/bindings";
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuSeparator,
  ContextMenuSub,
  ContextMenuSubContent,
  ContextMenuSubTrigger,
  ContextMenuTrigger,
} from "@/components/ui/context-menu";
import ActionMenuItem from "@/components/graph/ActionMenuItem.vue";
import { shortSha } from "@/lib/refNames";
import {
  copyCommitMessage,
  copyText,
  createBranchHere,
  createTagAt,
  currentBranch,
  dirtyTreeReason,
  isDetached,
  mergeInto,
  pickCommit,
  pickReason,
  removeTag,
  requestCheckout,
  resetReason,
  resetTo,
  revisionOf,
} from "@/lib/repoActions";
import type { RepoSession } from "@/stores/sessions";

/** Right-click menu for one commit row. The slot is the row itself, which becomes the trigger. */
const props = defineProps<{
  row: GraphRow;
  session: RepoSession;
}>();

const sha = computed(() => shortSha(props.row.oid));
const localBranches = computed(() => props.row.refs.filter((r) => r.kind.type === "localBranch"));
const tags = computed(() => props.row.refs.filter((r) => r.kind.type === "tag"));
/** The one local branch on this row, when there is exactly one (a single item instead of a submenu). */
const singleBranch = computed<RefLabel | null>(() => (localBranches.value.length === 1 ? (localBranches.value[0] ?? null) : null));
const singleTag = computed<RefLabel | null>(() => (tags.value.length === 1 ? (tags.value[0] ?? null) : null));

const dirty = computed(() => dirtyTreeReason(props.session));
const branch = computed(() => currentBranch(props.session));

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
const mergeReason = computed(() => {
  if (props.row.isHead) return "This commit is HEAD already.";
  if (isDetached(props.session)) return "HEAD is detached. Check out a branch to merge into first.";
  return dirty.value;
});
</script>

<template>
  <ContextMenu>
    <ContextMenuTrigger as-child>
      <slot />
    </ContextMenuTrigger>
    <ContextMenuContent class="min-w-56">
      <ActionMenuItem v-if="localBranches.length === 0" :reason="null" @select="requestCheckout(session.key, { kind: 'revision', revision: row.oid, label: sha })">
        Checkout {{ sha }}
      </ActionMenuItem>
      <ActionMenuItem v-else-if="singleBranch" :reason="checkoutReason(singleBranch)" @select="requestCheckout(session.key, { kind: 'local', name: singleBranch.short })">
        Checkout {{ singleBranch.short }}
      </ActionMenuItem>
      <ContextMenuSub v-else>
        <ContextMenuSubTrigger class="text-base">Checkout</ContextMenuSubTrigger>
        <ContextMenuSubContent>
          <ActionMenuItem v-for="b in localBranches" :key="b.fullName" :reason="checkoutReason(b)" @select="requestCheckout(session.key, { kind: 'local', name: b.short })">
            {{ b.short }}
          </ActionMenuItem>
        </ContextMenuSubContent>
      </ContextMenuSub>

      <ActionMenuItem @select="createBranchHere(session.key, row.oid, sha)">Create Branch Here…</ActionMenuItem>

      <ActionMenuItem :reason="mergeReason" @select="mergeInto(session.key, mergeSource, mergeLabel)">
        Merge {{ mergeLabel }} into {{ branch ?? "current branch" }}
      </ActionMenuItem>

      <ActionMenuItem :reason="pickReason(session, row, 'cherry-pick')" @select="pickCommit(session.key, row, 'cherry-pick')">
        Cherry-pick {{ sha }} onto {{ branch ?? "HEAD" }}
      </ActionMenuItem>
      <ActionMenuItem :reason="pickReason(session, row, 'revert')" @select="pickCommit(session.key, row, 'revert')">
        Revert {{ sha }}
      </ActionMenuItem>

      <ContextMenuSub>
        <ContextMenuSubTrigger class="text-base" :disabled="resetReason(session) !== null">Reset {{ branch ?? "HEAD" }} to {{ sha }}</ContextMenuSubTrigger>
        <ContextMenuSubContent class="min-w-64">
          <ActionMenuItem :reason="resetReason(session)" @select="resetTo(session.key, row.oid, sha, 'soft')">Soft: keep the changes staged…</ActionMenuItem>
          <ActionMenuItem :reason="resetReason(session)" @select="resetTo(session.key, row.oid, sha, 'mixed')">Mixed: keep the changes unstaged…</ActionMenuItem>
          <ActionMenuItem :reason="resetReason(session)" destructive @select="resetTo(session.key, row.oid, sha, 'hard')">Hard: discard every change…</ActionMenuItem>
        </ContextMenuSubContent>
      </ContextMenuSub>

      <ContextMenuSeparator />

      <ActionMenuItem @select="createTagAt(session.key, row.oid, sha)">Add Tag…</ActionMenuItem>
      <ActionMenuItem v-if="singleTag" destructive @select="removeTag(session.key, singleTag.short)">
        Remove Tag {{ singleTag.short }}
      </ActionMenuItem>
      <ContextMenuSub v-else-if="tags.length > 1">
        <ContextMenuSubTrigger class="text-base">Remove Tag</ContextMenuSubTrigger>
        <ContextMenuSubContent>
          <ActionMenuItem v-for="t in tags" :key="t.fullName" destructive @select="removeTag(session.key, t.short)">
            {{ t.short }}
          </ActionMenuItem>
        </ContextMenuSubContent>
      </ContextMenuSub>

      <ContextMenuSeparator />

      <ActionMenuItem @select="copyText(row.oid, 'the SHA')">Copy SHA</ActionMenuItem>
      <ActionMenuItem @select="copyText(sha, 'the short SHA')">Copy Short SHA</ActionMenuItem>
      <ActionMenuItem @select="copyCommitMessage(session.key, row.oid)">Copy Commit Message</ActionMenuItem>
    </ContextMenuContent>
  </ContextMenu>
</template>
