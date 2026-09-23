<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from "vue";
import type { UnlistenFn } from "@tauri-apps/api/event";
import { FolderOpenIcon, TriangleAlertIcon, XIcon } from "@lucide/vue";
import { Button } from "@/components/ui/button";
import { ResizableHandle, ResizablePanel, ResizablePanelGroup } from "@/components/ui/resizable";
import { TooltipProvider } from "@/components/ui/tooltip";
import AboutDialog from "@/components/dialogs/AboutDialog.vue";
import ConfirmDialog from "@/components/dialogs/ConfirmDialog.vue";
import CreateBranchDialog from "@/components/dialogs/CreateBranchDialog.vue";
import CreateTagDialog from "@/components/dialogs/CreateTagDialog.vue";
import AddRepoDialog from "@/components/shell/AddRepoDialog.vue";
import AppToolbar from "@/components/shell/AppToolbar.vue";
import BranchesPanel from "@/components/shell/BranchesPanel.vue";
import ConsolePanel from "@/components/shell/ConsolePanel.vue";
import EmptyState from "@/components/shell/EmptyState.vue";
import RepositoriesPanel from "@/components/shell/RepositoriesPanel.vue";
import RepoTabStrip from "@/components/shell/RepoTabStrip.vue";
import RepoWorkspace from "@/components/shell/RepoWorkspace.vue";
import StatusBar from "@/components/shell/StatusBar.vue";
import { errorMessage } from "@/lib/ipc";
import { installMenuActions } from "@/lib/menuActions";
import { useDialogsStore } from "@/stores/dialogs";
import { useHostStore } from "@/stores/host";
import { useSessionsStore } from "@/stores/sessions";
import { useSettingsStore } from "@/stores/settings";
import { useUiStateStore } from "@/stores/uiState";

const settings = useSettingsStore();
const uiState = useUiStateStore();
const host = useHostStore();
const sessions = useSessionsStore();
const dialogs = useDialogsStore();

const ready = ref(false);
let unlistenMenu: UnlistenFn | null = null;

onMounted(async () => {
  // The console listener must be live before the first repoOpen emits probe commands.
  await sessions.init();
  unlistenMenu = await installMenuActions();
  const results = await Promise.allSettled([settings.load(), uiState.load(), host.load()]);
  for (const r of results) {
    if (r.status === "rejected") sessions.notify(errorMessage(r.reason));
  }
  ready.value = true;
  await sessions.restore();
});

onBeforeUnmount(() => {
  unlistenMenu?.();
  unlistenMenu = null;
});

// Splitter sizes are read once when the shell first renders; drags write them back.
const leftWidth = computed(() => uiState.panelSize("leftWidth"));
const reposHeight = computed(() => uiState.panelSize("reposHeight"));
const consoleHeight = computed(() => uiState.panelSize("consoleHeight"));
const consoleCollapsed = computed(() => uiState.consoleCollapsed);
const sidebarCollapsed = computed(() => uiState.sidebarCollapsed);

function onBodyLayout(sizes: number[]): void {
  // With the sidebar hidden the group has one panel and reports [100]; do not save that.
  if (sizes.length === 2) uiState.setPanelSize("leftWidth", sizes[0] as number);
}

/** Runs the notice's button (for example "Abort merge") and dismisses the notice. */
function runNoticeAction(): void {
  const action = sessions.noticeAction;
  sessions.dismissNotice();
  if (action) void action.run();
}

function onSidebarLayout(sizes: number[]): void {
  if (sizes.length === 2) uiState.setPanelSize("reposHeight", sizes[0] as number);
}

function onMainLayout(sizes: number[]): void {
  // With the console collapsed the group has one panel and reports [100]; do not save that.
  if (sizes.length === 2) uiState.setPanelSize("consoleHeight", sizes[1] as number);
}

async function refresh(): Promise<void> {
  const key = sessions.activeKey;
  if (key === null) return;
  await sessions.refreshSession(key);
}
</script>

<template>
  <TooltipProvider :delay-duration="300">
    <div class="flex h-screen w-screen flex-col overflow-hidden bg-background text-foreground">
      <AppToolbar @add-repo="dialogs.addRepoOpen = true" @refresh="refresh" />

      <div
        v-if="sessions.notice"
        class="flex shrink-0 items-center gap-2 border-b border-deleted/40 bg-deleted/10 px-3 py-1 text-base text-deleted-fg"
        role="alert"
      >
        <TriangleAlertIcon class="size-3.5 shrink-0" />
        <span class="truncate">{{ sessions.notice }}</span>
        <button
          v-if="sessions.noticeAction"
          type="button"
          class="ml-1 shrink-0 rounded border border-deleted/40 px-1.5 text-xs font-medium hover:bg-deleted/20"
          @click="runNoticeAction"
        >
          {{ sessions.noticeAction.label }}
        </button>
        <button type="button" class="ml-auto rounded p-0.5 hover:bg-deleted/20" aria-label="Dismiss" @click="sessions.dismissNotice()">
          <XIcon class="size-3.5" />
        </button>
      </div>

      <div
        v-for="(warning, i) in host.warnings"
        :key="i"
        class="flex shrink-0 items-center gap-2 border-b border-modified/40 bg-modified/10 px-3 py-1 text-base text-modified-fg"
        role="status"
      >
        <TriangleAlertIcon class="size-3.5 shrink-0" />
        <span class="truncate">{{ warning }}</span>
        <button type="button" class="ml-auto rounded p-0.5 hover:bg-modified/20" aria-label="Dismiss" @click="host.dismissWarning(i)">
          <XIcon class="size-3.5" />
        </button>
      </div>

      <div v-if="!ready" class="flex min-h-0 flex-1 items-center justify-center text-base text-muted-foreground">
        Loading…
      </div>

      <ResizablePanelGroup v-else direction="horizontal" class="min-h-0 flex-1" @layout="onBodyLayout">
        <!-- View > Toggle Sidebar removes the panel and its handle; `order` keeps the splitter's layout stable. -->
        <template v-if="!sidebarCollapsed">
          <ResizablePanel :order="1" :default-size="leftWidth" :min-size="12" :max-size="45">
            <ResizablePanelGroup direction="vertical" @layout="onSidebarLayout">
              <ResizablePanel :default-size="reposHeight" :min-size="15">
                <RepositoriesPanel @add-repo="dialogs.addRepoOpen = true" />
              </ResizablePanel>
              <ResizableHandle />
              <ResizablePanel :default-size="100 - reposHeight" :min-size="15">
                <BranchesPanel />
              </ResizablePanel>
            </ResizablePanelGroup>
          </ResizablePanel>
          <ResizableHandle />
        </template>

        <ResizablePanel :order="2" :min-size="40">
          <div class="flex h-full min-h-0 flex-col">
            <ResizablePanelGroup direction="vertical" class="min-h-0 flex-1" @layout="onMainLayout">
              <ResizablePanel :order="1" :default-size="consoleCollapsed ? 100 : 100 - consoleHeight" :min-size="25">
                <div class="flex h-full min-h-0 flex-col">
                  <RepoTabStrip />
                  <div class="relative min-h-0 flex-1">
                    <EmptyState
                      v-if="sessions.list.length === 0"
                      message="Open a repository to get started."
                    >
                      <Button variant="outline" size="sm" @click="dialogs.addRepoOpen = true">
                        <FolderOpenIcon />
                        Open Repository
                      </Button>
                    </EmptyState>
                    <!-- Every open session stays mounted; v-show keeps its state across tab switches. -->
                    <div
                      v-for="session in sessions.list"
                      :key="session.key"
                      v-show="session.key === sessions.activeKey"
                      class="absolute inset-0"
                    >
                      <RepoWorkspace :session="session" />
                    </div>
                  </div>
                </div>
              </ResizablePanel>
              <template v-if="!consoleCollapsed">
                <ResizableHandle />
                <ResizablePanel :order="2" :default-size="consoleHeight" :min-size="8">
                  <ConsolePanel :session="sessions.active" :collapsed="false" />
                </ResizablePanel>
              </template>
            </ResizablePanelGroup>
            <div v-if="consoleCollapsed" class="shrink-0 border-t border-border">
              <ConsolePanel :session="sessions.active" :collapsed="true" />
            </div>
          </div>
        </ResizablePanel>
      </ResizablePanelGroup>

      <StatusBar />
    </div>

    <AddRepoDialog v-model:open="dialogs.addRepoOpen" />
    <CreateBranchDialog />
    <CreateTagDialog />
    <ConfirmDialog />
    <AboutDialog />
  </TooltipProvider>
</template>
