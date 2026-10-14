<script setup lang="ts">
import { ResizableHandle, ResizablePanel, ResizablePanelGroup } from "@/components/ui/resizable";
import CommitGraphView from "@/components/graph/CommitGraphView.vue";
import DiffViewer from "@/components/diff/DiffViewer.vue";
import FilesPanel from "@/components/files/FilesPanel.vue";
import OperationBanner from "@/components/shell/OperationBanner.vue";
import type { RepoSession } from "@/stores/sessions";
import { useUiStateStore } from "@/stores/uiState";

defineProps<{
  session: RepoSession;
}>();

const uiState = useUiStateStore();

// Sizes are read once at mount; every workspace shares the same saved percentages.
const diffHeight = uiState.panelSize("diffHeight");
const filesWidth = uiState.panelSize("filesWidth");

function onRowsLayout(sizes: number[]): void {
  if (sizes.length === 2) uiState.setPanelSize("diffHeight", sizes[1] as number);
}

function onTopLayout(sizes: number[]): void {
  if (sizes.length === 2) uiState.setPanelSize("filesWidth", sizes[1] as number);
}
</script>

<template>
  <div class="flex h-full min-h-0 flex-col">
    <!-- Only while a merge, rebase, cherry-pick or revert has stopped part way. -->
    <OperationBanner :session="session" />
    <ResizablePanelGroup direction="vertical" class="min-h-0 flex-1" @layout="onRowsLayout">
      <ResizablePanel :default-size="100 - diffHeight" :min-size="15">
        <ResizablePanelGroup direction="horizontal" @layout="onTopLayout">
          <ResizablePanel :default-size="100 - filesWidth" :min-size="20">
            <CommitGraphView :session="session" />
          </ResizablePanel>
          <ResizableHandle />
          <ResizablePanel :default-size="filesWidth" :min-size="15">
            <FilesPanel :session="session" />
          </ResizablePanel>
        </ResizablePanelGroup>
      </ResizablePanel>
      <ResizableHandle />
      <ResizablePanel :default-size="diffHeight" :min-size="10">
        <DiffViewer :session="session" />
      </ResizablePanel>
    </ResizablePanelGroup>
  </div>
</template>
