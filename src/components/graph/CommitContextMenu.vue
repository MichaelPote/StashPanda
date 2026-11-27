<script setup lang="ts">
import type { GraphRow } from "@/lib/bindings";
import { ContextMenu, ContextMenuContent, ContextMenuTrigger } from "@/components/ui/context-menu";
import CommitActionItems from "@/components/graph/CommitActionItems.vue";
import CommitMenuItems from "@/components/graph/CommitMenuItems.vue";
import type { RepoSession } from "@/stores/sessions";

/**
 * Right-click menu for one commit row. The slot is the row itself, which becomes the trigger.
 * Its two parts are shared: Checkout, Create Branch Here and Merge with the menu of a tag that
 * only a remote has (CommitActionItems.vue), and everything after Merge with the branch and tag
 * menus as well (CommitMenuItems.vue), so that a commit, a branch and a tag offer the same
 * operations on the commit.
 */
defineProps<{
  row: GraphRow;
  session: RepoSession;
}>();
</script>

<template>
  <ContextMenu>
    <ContextMenuTrigger as-child>
      <slot />
    </ContextMenuTrigger>
    <ContextMenuContent class="min-w-56">
      <CommitActionItems :session="session" :row="row" />
      <CommitMenuItems :session="session" :oid="row.oid" :row="row" />
    </ContextMenuContent>
  </ContextMenu>
</template>
