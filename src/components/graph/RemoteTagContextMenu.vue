<script setup lang="ts">
import { computed } from "vue";
import type { GraphRow } from "@/lib/bindings";
import { ContextMenu, ContextMenuContent, ContextMenuSeparator, ContextMenuTrigger } from "@/components/ui/context-menu";
import ActionMenuItem from "@/components/graph/ActionMenuItem.vue";
import CommitActionItems from "@/components/graph/CommitActionItems.vue";
import CommitMenuItems from "@/components/graph/CommitMenuItems.vue";
import { busyReason, fetchTag, removeRemoteOnlyTag } from "@/lib/repoActions";
import { useSessionsStore } from "@/stores/sessions";

/**
 * Right-click menu for a tag only a remote has: fetch it here, or remove it from the remote. The
 * local tag actions (rename, move, push) need the tag to be here first.
 *
 * On a pill in the graph it goes on with the commit row's own items (Checkout, Create Branch
 * Here, Merge, Cherry-pick and the rest), which name the commit by its ID, since Git here does
 * not know the tag's name. The Branches panel passes no row and gets the two tag items only.
 */
const props = defineProps<{
  sessionKey: string;
  name: string;
  /** The remotes that have the tag on this commit. */
  remotes: string[];
  /** The commit's row, for a pill in the graph. */
  row?: GraphRow | null;
}>();

const sessions = useSessionsStore();
const session = computed(() => sessions.sessions.get(props.sessionKey) ?? null);
const reason = computed(() => (session.value ? busyReason(session.value) : "Open a repository first."));
</script>

<template>
  <ContextMenu>
    <ContextMenuTrigger as-child>
      <slot />
    </ContextMenuTrigger>
    <ContextMenuContent>
      <ActionMenuItem v-for="remote in remotes" :key="`fetch-${remote}`" :reason="reason" @select="fetchTag(sessionKey, remote, name)">
        Fetch Tag from {{ remote }}
      </ActionMenuItem>
      <ContextMenuSeparator />
      <ActionMenuItem
        v-for="remote in remotes"
        :key="`remove-${remote}`"
        :reason="reason"
        destructive
        @select="removeRemoteOnlyTag(sessionKey, remote, name)"
      >
        Remove from {{ remote }}…
      </ActionMenuItem>
      <template v-if="session && row">
        <ContextMenuSeparator />
        <CommitActionItems :session="session" :row="row" />
        <CommitMenuItems :session="session" :oid="row.oid" :row="row" />
      </template>
    </ContextMenuContent>
  </ContextMenu>
</template>
