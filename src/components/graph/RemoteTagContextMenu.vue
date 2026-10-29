<script setup lang="ts">
import { computed } from "vue";
import { ContextMenu, ContextMenuContent, ContextMenuSeparator, ContextMenuTrigger } from "@/components/ui/context-menu";
import ActionMenuItem from "@/components/graph/ActionMenuItem.vue";
import { busyReason, fetchTag, removeRemoteOnlyTag } from "@/lib/repoActions";
import { useSessionsStore } from "@/stores/sessions";

/**
 * Right-click menu for a tag only a remote has: fetch it here, or remove it from the remote. The
 * local tag actions (rename, move, push) need the tag to be here first.
 */
const props = defineProps<{
  sessionKey: string;
  name: string;
  /** The remotes that have the tag on this commit. */
  remotes: string[];
}>();

const sessions = useSessionsStore();
const reason = computed(() => {
  const session = sessions.sessions.get(props.sessionKey);
  return session ? busyReason(session) : "Open a repository first.";
});
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
    </ContextMenuContent>
  </ContextMenu>
</template>
