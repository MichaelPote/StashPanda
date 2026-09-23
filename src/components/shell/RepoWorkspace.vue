<script setup lang="ts">
import { ResizableHandle, ResizablePanel, ResizablePanelGroup } from "@/components/ui/resizable";
import CommitGraphView from "@/components/graph/CommitGraphView.vue";
import DiffViewer from "@/components/diff/DiffViewer.vue";
import FilesPanel from "@/components/files/FilesPanel.vue";
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
  <ResizablePanelGroup direction="vertical" class="h-full min-h-0" @layout="onRowsLayout">
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
</template>
