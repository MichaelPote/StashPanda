<script setup lang="ts">
import { computed } from "vue";
import {
  ChevronDownIcon,
  ChevronRightIcon,
  FolderGit2Icon,
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
import { transportLabel } from "@/lib/format";
import { cn } from "@/lib/utils";
import { repoDisplayName, useLibraryStore, type LibraryNode } from "@/stores/library";
import { useSessionsStore } from "@/stores/sessions";

const props = defineProps<{
  node: LibraryNode;
  depth: number;
}>();

const emit = defineEmits<{
  open: [key: string];
  remove: [key: string];
}>();

const library = useLibraryStore();
const sessions = useSessionsStore();

const indent = computed(() => ({ paddingLeft: `${8 + props.depth * 14}px` }));

const isOpen = computed(() => props.node.kind === "repo" && sessions.sessions.has(props.node.id));
const isActive = computed(() => props.node.kind === "repo" && sessions.activeKey === props.node.id);
const isOpening = computed(() => props.node.kind === "repo" && sessions.opening.has(props.node.id));

function toggleGroup(): void {
  if (props.node.kind !== "group") return;
  void library.setGroupCollapsed(props.node.id, !(props.node.group.collapsed ?? false));
}
</script>

<template>
  <template v-if="node.kind === 'group'">
    <button
      type="button"
      class="flex h-(--row-h) w-full cursor-pointer items-center gap-1 pr-2 text-left text-base hover:bg-row-hover"
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

  <ContextMenu v-else>
    <ContextMenuTrigger as-child>
      <button
        type="button"
        :class="
          cn(
            'flex h-(--row-h) w-full cursor-pointer items-center gap-1.5 pr-2 text-left text-base hover:bg-row-hover',
            isActive && 'row-selected',
            isOpen && !isActive && 'font-medium',
          )
        "
        :style="indent"
        :title="node.repo.path"
        @click="emit('open', node.id)"
        @dblclick.prevent
      >
        <LoaderCircleIcon v-if="isOpening" class="size-3.5 shrink-0 animate-spin text-muted-foreground" />
        <FolderGit2Icon
          v-else
          :class="cn('size-3.5 shrink-0', isOpen ? 'text-foreground' : 'text-muted-foreground')"
        />
        <span class="truncate">{{ repoDisplayName(node.repo) }}</span>
        <Badge
          as="span"
          variant="outline"
          class="ml-auto h-4 shrink-0 rounded-sm border-transparent bg-neutral px-1.5 text-[0.7rem] font-normal text-muted-foreground"
        >
          {{ transportLabel(node.repo.resolvedTransport) }}
        </Badge>
      </button>
    </ContextMenuTrigger>
    <ContextMenuContent class="min-w-44">
      <ContextMenuItem class="text-base" @select="emit('open', node.id)">Open</ContextMenuItem>
      <ContextMenuSeparator />
      <ContextMenuItem class="text-base" variant="destructive" @select="emit('remove', node.id)">
        Remove from library
      </ContextMenuItem>
    </ContextMenuContent>
  </ContextMenu>
</template>
