<script setup lang="ts">
import { PlusIcon } from "@lucide/vue";
import { Button } from "@/components/ui/button";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import EmptyState from "@/components/shell/EmptyState.vue";
import LibraryTreeNode from "@/components/shell/LibraryTreeNode.vue";
import PanelHeader from "@/components/shell/PanelHeader.vue";
import { openRepository, removeFromLibrary } from "@/lib/repoActions";
import { useDialogsStore } from "@/stores/dialogs";
import { useLibraryStore } from "@/stores/library";

const emit = defineEmits<{
  addRepo: [];
}>();

const dialogs = useDialogsStore();
const library = useLibraryStore();

async function open(key: string): Promise<void> {
  await openRepository(key);
}

</script>

<template>
  <section class="area flex h-full min-h-0 flex-col">
    <PanelHeader title="Repositories">
      <template #actions>
        <Tooltip>
          <TooltipTrigger as-child>
            <Button variant="ghost" size="icon-xs" class="size-5" @click="emit('addRepo')">
              <PlusIcon />
              <span class="sr-only">Add repository</span>
            </Button>
          </TooltipTrigger>
          <TooltipContent side="bottom">Add repository</TooltipContent>
        </Tooltip>
      </template>
    </PanelHeader>

    <EmptyState v-if="library.isEmpty" message="No repositories in the library yet.">
      <Button variant="outline" size="xs" @click="emit('addRepo')">Add repository…</Button>
    </EmptyState>

    <ScrollArea v-else class="min-h-0 flex-1">
      <!-- Whole pixels (py-1 is 3.5px), so the rows' 12px icons start on whole pixels; see index.css. -->
      <div class="py-[4px]">
        <LibraryTreeNode
          v-for="node in library.tree"
          :key="node.id"
          :node="node"
          :depth="0"
          @open="open"
          @edit="dialogs.editRepo"
          @remove="removeFromLibrary"
        />
      </div>
    </ScrollArea>
  </section>
</template>
