<script setup lang="ts">
import { computed } from "vue";
import {
  ChevronDownIcon,
  ChevronRightIcon,
  FolderRootIcon,
  FolderIcon,
  FolderOpenIcon,
  LoaderCircleIcon,
} from "@lucide/vue";
import { Badge } from "@/components/ui/badge";
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuSeparator,
  ContextMenuTrigger,
} from "@/components/ui/context-menu";
import HueSubmenu from "@/components/ui/HueSubmenu.vue";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { shortOid, transportDetail, transportLabel } from "@/lib/format";
import { cn } from "@/lib/utils";
import { repoDisplayName, useLibraryStore, type LibraryNode } from "@/stores/library";
import { useSessionsStore } from "@/stores/sessions";

const props = defineProps<{
  node: LibraryNode;
  depth: number;
}>();

const emit = defineEmits<{
  open: [key: string];
  edit: [key: string];
  remove: [key: string];
}>();

const library = useLibraryStore();
const sessions = useSessionsStore();

const indent = computed(() => ({ paddingLeft: `${8 + props.depth * 14}px` }));

const isOpen = computed(() => props.node.kind === "repo" && sessions.sessions.has(props.node.id));
const isActive = computed(() => props.node.kind === "repo" && sessions.activeKey === props.node.id);
const isOpening = computed(() => props.node.kind === "repo" && sessions.opening.has(props.node.id));

/**
 * What the row shows on the right: the checked-out branch, or a detached HEAD's short commit ID
 * (`commit`), from the open tab when there is one, else from `library.heads`. Null when neither
 * knows, such as an SSH repository whose server is not connected; the row then shows the transport.
 */
const head = computed<{ branch: string | null; commit: string | null } | null>(() => {
  if (props.node.kind !== "repo") return null;
  const session = sessions.sessions.get(props.node.id);
  if (session?.connection && session.connection.state !== "ready") return null;
  // A tab restored at startup has no status until it is first shown.
  const status = session?.status;
  if (status) {
    if (!status.detached && status.branch) return { branch: status.branch, commit: null };
    return status.headOid ? { branch: null, commit: shortOid(status.headOid) } : null;
  }
  const read = library.heads.get(props.node.id);
  return read ? { branch: read.branch, commit: read.commit } : null;
});

function toggleGroup(): void {
  if (props.node.kind !== "group") return;
  void library.setGroupCollapsed(props.node.id, !(props.node.group.collapsed ?? false));
}
</script>

<template>
  <template v-if="node.kind === 'group'">
    <button
      type="button"
      class="flex h-(--row-h) w-full items-center gap-1 pr-2 text-left text-base hover:bg-row-hover"
      :style="indent"
      @click="toggleGroup"
    >
      <component
        :is="node.group.collapsed ? ChevronRightIcon : ChevronDownIcon"
        class="size-3.5 shrink-0 text-muted-foreground"
      />
      <component
        :is="node.group.collapsed ? FolderIcon : FolderOpenIcon"
        class="size-3.5 shrink-0 text-muted-foreground"
      />
      <span class="truncate font-medium">{{ node.group.name || "Group" }}</span>
      <span class="ml-auto text-[0.7rem] text-muted-foreground">{{ node.children.length }}</span>
    </button>
    <template v-if="!node.group.collapsed">
      <LibraryTreeNode
        v-for="child in node.children"
        :key="child.id"
        :node="child"
        :depth="depth + 1"
        @open="emit('open', $event)"
        @edit="emit('edit', $event)"
        @remove="emit('remove', $event)"
      />
      <p
        v-if="node.children.length === 0"
        class="h-(--row-h) truncate text-xs leading-(--row-h) text-muted-foreground italic"
        :style="{ paddingLeft: `${8 + (depth + 1) * 14 + 18}px` }"
      >
        Empty group
      </p>
    </template>
  </template>

  <!-- The tooltip says where the repository is and how it is reached: its path as entered, then
       the transport. -->
  <Tooltip v-else>
    <ContextMenu>
      <ContextMenuTrigger as-child>
        <TooltipTrigger as-child>
          <button
            type="button"
            :class="
              cn(
                'flex h-(--row-h) w-full items-center gap-1.5 pr-2 text-left text-base hover:bg-row-hover',
                isActive && 'row-selected',
                isOpen && !isActive && 'font-medium',
              )
            "
            :style="indent"
            @click="emit('open', node.id)"
            @dblclick.prevent
          >
            <LoaderCircleIcon v-if="isOpening" class="size-3.5 shrink-0 animate-spin text-muted-foreground" />
            <!-- A repository with a colour shows it here, open or not. -->
            <FolderRootIcon
              v-else
              :class="cn('size-3.5 shrink-0', isOpen ? 'text-foreground' : 'text-muted-foreground')"
              :style="node.repo.colour != null ? { color: `var(--graph-lane-${node.repo.colour})` } : undefined"
              :data-repo-colour="node.repo.colour ?? undefined"
            />
            <span class="truncate">{{ repoDisplayName(node.repo) }}</span>
            <!-- The name keeps most of the row: the branch shrinks first. -->
            <span v-if="head" :class="cn('ml-auto min-w-0 shrink-[4] truncate pl-1 text-[0.7rem] text-muted-foreground', head.commit && 'font-mono')">
              {{ head.branch ?? head.commit }}
            </span>
            <Badge
              v-else
              as="span"
              variant="outline"
              class="ml-auto h-4 shrink-0 rounded-sm border-transparent bg-neutral px-1.5 text-[0.7rem] font-normal text-muted-foreground"
            >
              {{ transportLabel(node.repo.resolvedTransport) }}
            </Badge>
          </button>
        </TooltipTrigger>
      </ContextMenuTrigger>
      <ContextMenuContent class="min-w-44">
        <ContextMenuItem class="text-base" @select="emit('open', node.id)">Open</ContextMenuItem>
        <ContextMenuItem class="text-base" @select="emit('edit', node.id)">Edit…</ContextMenuItem>
        <!-- Its icon here, and the interface's accent while its tab is active. -->
        <HueSubmenu :chosen="node.repo.colour ?? null" @choose="(c) => library.setRepoColour(node.id, c)" />
        <ContextMenuSeparator />
        <ContextMenuItem class="text-base" variant="destructive" @select="emit('remove', node.id)">
          Remove from library
        </ContextMenuItem>
      </ContextMenuContent>
    </ContextMenu>
    <TooltipContent side="right">
      <!-- A column: the tooltip lays its content out in a row. A path has few places to wrap. -->
      <div class="flex flex-col [overflow-wrap:anywhere]">
        <span class="font-medium">{{ node.repo.path }}</span>
        <span>{{ transportDetail(node.repo.resolvedTransport) }}</span>
      </div>
    </TooltipContent>
  </Tooltip>
</template>
