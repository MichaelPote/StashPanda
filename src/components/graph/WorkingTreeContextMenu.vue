<script setup lang="ts">
import { ContextMenu, ContextMenuContent, ContextMenuSeparator, ContextMenuTrigger } from "@/components/ui/context-menu";
import ActionMenuItem from "@/components/graph/ActionMenuItem.vue";
import { applyStashReason, openApplyStashDialog, openCommitDialog, openStashDialog, stashReason } from "@/lib/repoActions";
import type { RepoSession } from "@/stores/sessions";

/** Right-click menu for the Working Tree row. The slot is the row itself, which becomes the trigger. */
defineProps<{
  session: RepoSession;
}>();
</script>

<template>
  <ContextMenu>
    <ContextMenuTrigger as-child>
      <slot />
    </ContextMenuTrigger>
    <ContextMenuContent class="min-w-56">
      <ActionMenuItem @select="openCommitDialog(session)">Commit…</ActionMenuItem>
      <ContextMenuSeparator />
      <ActionMenuItem :reason="stashReason(session)" @select="openStashDialog(session)">Stash Changes…</ActionMenuItem>
      <ActionMenuItem :reason="applyStashReason(session)" @select="openApplyStashDialog(session)">Apply Stash…</ActionMenuItem>
    </ContextMenuContent>
  </ContextMenu>
</template>
