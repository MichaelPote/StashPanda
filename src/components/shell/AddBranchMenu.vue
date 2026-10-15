<script setup lang="ts">
import { computed } from "vue";
import { ContextMenu, ContextMenuContent, ContextMenuItem, ContextMenuTrigger } from "@/components/ui/context-menu";
import { shortSha } from "@/lib/refNames";
import { createBranchHere } from "@/lib/repoActions";
import { useSessionsStore } from "@/stores/sessions";

/**
 * The right-click menu of a heading in the Branches panel: a section, a remote or a branch folder.
 * "Add Branch…" opens the Create Branch dialog at the checked-out commit; from a folder, the new
 * branch's name starts with the folder's path (`feature/`). The slot is the trigger.
 */
const props = defineProps<{
  sessionKey: string;
  /** The start of the new branch's name, for a folder's heading. */
  prefix?: string;
}>();

const sessions = useSessionsStore();
const session = computed(() => sessions.sessions.get(props.sessionKey) ?? null);

function addBranch(): void {
  const s = session.value;
  if (!s) return;
  const head = s.status?.headOid ?? null;
  const label = s.status?.branch ?? (head ? shortSha(head) : "HEAD");
  void createBranchHere(props.sessionKey, "HEAD", label, props.prefix);
}
</script>

<template>
  <ContextMenu>
    <ContextMenuTrigger as-child>
      <slot />
    </ContextMenuTrigger>
    <ContextMenuContent class="min-w-44">
      <ContextMenuItem class="text-base" data-add-branch @select="addBranch">Add Branch…</ContextMenuItem>
    </ContextMenuContent>
  </ContextMenu>
</template>
