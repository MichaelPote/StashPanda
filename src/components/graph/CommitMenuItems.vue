<script setup lang="ts">
import { computed } from "vue";
import type { GraphRow } from "@/lib/bindings";
import { ContextMenuSeparator, ContextMenuSub, ContextMenuSubContent, ContextMenuSubTrigger } from "@/components/ui/context-menu";
import ActionMenuItem from "@/components/graph/ActionMenuItem.vue";
import LaneBranchName from "@/components/graph/LaneBranchName.vue";
import { shortSha } from "@/lib/refNames";
import { copyCommitMessage, copyText, createTagAt, currentBranch, pickCommit, pickReason, removeTag, resetReason, resetTo } from "@/lib/repoActions";
import type { RepoSession } from "@/stores/sessions";

/**
 * The items for one commit that the commit row's menu and the branch and tag menus share:
 * cherry-pick, revert and reset, then tags, then copying. Cherry-pick, revert and reset need the
 * commit's row in the graph, so they are left out without one, as in the Branches panel, whose
 * branch may not be among the loaded rows.
 */
const props = defineProps<{
  session: RepoSession;
  oid: string;
  row?: GraphRow | null;
}>();

const sha = computed(() => shortSha(props.oid));
const branch = computed(() => currentBranch(props.session));
/** The checked-out branch's full name, for its lane colour. */
const branchFullName = computed(() => (branch.value ? `refs/heads/${branch.value}` : ""));
/** The tags on this commit, by short name. */
const tags = computed(() =>
  (props.session.refs?.refs ?? []).filter((r) => r.kind.type === "tag" && r.target === props.oid).map((r) => r.short),
);
</script>

<template>
  <template v-if="row">
    <!-- Labels naming a branch are one <span>, because the item is a flex row whose gap would replace the spaces between words. -->
    <ActionMenuItem :reason="pickReason(session, row, 'cherry-pick')" @select="pickCommit(session.key, row, 'cherry-pick')">
      <span>
        Cherry-pick {{ sha }} onto
        <LaneBranchName v-if="branch" :session="session" :full-name="branchFullName" :name="branch" />
        <template v-else>HEAD</template>
      </span>
    </ActionMenuItem>
    <ActionMenuItem :reason="pickReason(session, row, 'revert')" @select="pickCommit(session.key, row, 'revert')">
      Revert {{ sha }}
    </ActionMenuItem>

    <ContextMenuSub>
      <ContextMenuSubTrigger class="text-base" :disabled="resetReason(session) !== null">
        <span>
          Reset
          <LaneBranchName v-if="branch" :session="session" :full-name="branchFullName" :name="branch" />
          <template v-else>HEAD</template>
          to {{ sha }}
        </span>
      </ContextMenuSubTrigger>
      <ContextMenuSubContent class="min-w-64">
        <ActionMenuItem :reason="resetReason(session)" @select="resetTo(session.key, oid, sha, 'soft')">Soft: keep the changes staged…</ActionMenuItem>
        <ActionMenuItem :reason="resetReason(session)" @select="resetTo(session.key, oid, sha, 'mixed')">Mixed: keep the changes unstaged…</ActionMenuItem>
        <ActionMenuItem :reason="resetReason(session)" destructive @select="resetTo(session.key, oid, sha, 'hard')">Hard: discard every change…</ActionMenuItem>
      </ContextMenuSubContent>
    </ContextMenuSub>

    <ContextMenuSeparator />
  </template>

  <ActionMenuItem @select="createTagAt(session.key, oid, sha)">Add Tag…</ActionMenuItem>
  <ActionMenuItem v-if="tags.length === 1" destructive @select="removeTag(session.key, tags[0]!)">
    Remove Tag {{ tags[0] }}…
  </ActionMenuItem>
  <ContextMenuSub v-else-if="tags.length > 1">
    <ContextMenuSubTrigger class="text-base">Remove Tag</ContextMenuSubTrigger>
    <ContextMenuSubContent>
      <ActionMenuItem v-for="t in tags" :key="t" destructive @select="removeTag(session.key, t)">
        {{ t }}…
      </ActionMenuItem>
    </ContextMenuSubContent>
  </ContextMenuSub>

  <ContextMenuSeparator />

  <ActionMenuItem @select="copyText(oid, 'the SHA')">Copy SHA</ActionMenuItem>
  <ActionMenuItem @select="copyText(sha, 'the short SHA')">Copy Short SHA</ActionMenuItem>
  <ActionMenuItem @select="copyCommitMessage(session.key, oid)">Copy Commit Message</ActionMenuItem>
</template>
