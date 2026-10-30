<script setup lang="ts">
import type { StashEntry } from "@/lib/bindings";
import { ContextMenu, ContextMenuContent, ContextMenuItem, ContextMenuSeparator, ContextMenuTrigger } from "@/components/ui/context-menu";
import { applyStash, dropStash, popStash } from "@/lib/repoActions";
import { stashParts } from "@/lib/stash";

/** Right-click menu for a stash, on its row in the Branches panel and its pill in the commit graph. */
defineProps<{
  sessionKey: string;
  stash: StashEntry;
}>();
</script>

<template>
  <ContextMenu>
    <ContextMenuTrigger as-child>
      <slot />
    </ContextMenuTrigger>
    <ContextMenuContent class="min-w-52">
      <ContextMenuItem class="text-base" @select="applyStash(sessionKey, stash)">Apply (keep the stash)</ContextMenuItem>
      <ContextMenuItem class="text-base" @select="popStash(sessionKey, stash)">Pop (apply, then drop)</ContextMenuItem>
      <ContextMenuSeparator />
      <ContextMenuItem class="text-base" variant="destructive" @select="dropStash(sessionKey, stash, stashParts(stash).text)">
        Drop…
      </ContextMenuItem>
    </ContextMenuContent>
  </ContextMenu>
</template>
