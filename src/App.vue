<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, reactive, ref } from "vue";
import type { UnlistenFn } from "@tauri-apps/api/event";
import { useEventListener } from "@vueuse/core";
import { FolderOpenIcon, TriangleAlertIcon, XIcon } from "@lucide/vue";
import { Button } from "@/components/ui/button";
import { ResizableHandle, ResizablePanel, ResizablePanelGroup } from "@/components/ui/resizable";
import { TooltipProvider } from "@/components/ui/tooltip";
import AboutDialog from "@/components/dialogs/AboutDialog.vue";
import ApplyStashDialog from "@/components/dialogs/ApplyStashDialog.vue";
import CommitDialog from "@/components/dialogs/CommitDialog.vue";
import ConfirmDialog from "@/components/dialogs/ConfirmDialog.vue";
import CreateBranchDialog from "@/components/dialogs/CreateBranchDialog.vue";
import CreateTagDialog from "@/components/dialogs/CreateTagDialog.vue";
import HostKeyDialog from "@/components/dialogs/HostKeyDialog.vue";
import RenameBranchDialog from "@/components/dialogs/RenameBranchDialog.vue";
import SshPasswordDialog from "@/components/dialogs/SshPasswordDialog.vue";
import PullDialog from "@/components/dialogs/PullDialog.vue";
import PushDialog from "@/components/dialogs/PushDialog.vue";
import StashDialog from "@/components/dialogs/StashDialog.vue";
import PassphraseDialog from "@/components/settings/PassphraseDialog.vue";
import SettingsDialog from "@/components/settings/SettingsDialog.vue";
import AddRepoDialog from "@/components/shell/AddRepoDialog.vue";
import AppToolbar from "@/components/shell/AppToolbar.vue";
import BranchesPanel from "@/components/shell/BranchesPanel.vue";
import ConsolePanel from "@/components/shell/ConsolePanel.vue";
import EmptyState from "@/components/shell/EmptyState.vue";
import RepositoriesPanel from "@/components/shell/RepositoriesPanel.vue";
import RepoTabStrip from "@/components/shell/RepoTabStrip.vue";
import RepoWorkspace from "@/components/shell/RepoWorkspace.vue";
import StatusBar from "@/components/shell/StatusBar.vue";
import { commitFileCount, isEditableTarget, isSelectAllKey } from "@/lib/files";
import { errorMessage } from "@/lib/ipc";
import { installMenuActions } from "@/lib/menuActions";
import {
  autoFetchDue,
  discardSelected,
  fetchAll,
  fileActionReason,
  openCommitDialog,
  openPullDialog,
  openStashDialog,
  pushCurrentBranch,
  stageSelected,
  syncCurrentBranch,
  unstageSelected,
} from "@/lib/repoActions";
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

// Automatic fetching, when `polling.autoFetchMinutes` is set: every half minute, fetch the tabs that are due.
const autoFetchTimer = setInterval(() => void autoFetchDue(), 30_000);
onBeforeUnmount(() => clearInterval(autoFetchTimer));

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

// Ctrl+A never selects the page text: outside a text field it means "select every file", which
// the active tab's Files panel does in its own listener. The native Edit > Select All menu item
// sends the same key press, so it takes this path too.
useEventListener(window, "keydown", (event: KeyboardEvent) => {
  if (isSelectAllKey(event) && !isEditableTarget(event.target)) event.preventDefault();
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

/** Files the Commit dialog would list for the active repository, shown on the toolbar's Commit button. */
const commitCount = computed(() => commitFileCount(sessions.active?.status ?? null));
/** Repository warnings dismissed in this run of the app, by repository key. */
const dismissedRepoWarnings = reactive(new Map<string, Set<string>>());
const repoWarnings = computed(() => {
  const s = sessions.active;
  if (!s) return [];
  const dismissed = dismissedRepoWarnings.get(s.key);
  return s.info.warnings.filter((w) => !dismissed?.has(w));
});
function dismissRepoWarning(warning: string): void {
  const key = sessions.activeKey;
  if (key === null) return;
  dismissedRepoWarnings.set(key, new Set([...(dismissedRepoWarnings.get(key) ?? []), warning]));
}

/** Stage, Unstage and Discard act on the files selected in the Files panel. */
const toolbarReasons = computed(() => ({
  stage: fileActionReason(sessions.active, "stage"),
  unstage: fileActionReason(sessions.active, "unstage"),
  discard: fileActionReason(sessions.active, "discard"),
}));
</script>

<template>
  <TooltipProvider :delay-duration="300">
    <div class="flex h-screen w-screen flex-col overflow-hidden bg-window text-foreground">
      <AppToolbar :has-repo="sessions.active !== null" :commit-count="commitCount" @add-repo="dialogs.showAddRepo()" @refresh="refresh" @commit="openCommitDialog(sessions.active)"
        @stash-save="openStashDialog(sessions.active)" @pull="openPullDialog(sessions.active)"
        @fetch="fetchAll(sessions.active)" @push="pushCurrentBranch(sessions.active)"
        @sync="syncCurrentBranch(sessions.active)" @stage="stageSelected(sessions.active)"
        @unstage="unstageSelected(sessions.active)" @discard="discardSelected(sessions.active)" :reasons="toolbarReasons"
      />

      <!-- An error notice: red, with a warning icon. Success messages show in the status bar instead. -->
      <div
        v-if="sessions.notice"
        class="flex shrink-0 items-center gap-2 border-b border-diff-del-mark bg-diff-del px-3 py-1 text-base text-destructive"
        role="alert"
      >
        <TriangleAlertIcon class="size-3.5 shrink-0" />
        <span class="truncate">{{ sessions.notice }}</span>
        <button
          v-if="sessions.noticeAction"
          type="button"
          class="ml-1 shrink-0 rounded border border-destructive/40 px-1.5 text-xs font-medium hover:bg-destructive/20"
          @click="runNoticeAction"
        >
          {{ sessions.noticeAction.label }}
        </button>
        <button type="button" class="ml-auto rounded p-0.5 hover:bg-destructive/20" aria-label="Dismiss" @click="sessions.dismissNotice()">
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

      <!-- What the active repository's connection warned about when it opened (Windows Git over a
           \\wsl.localhost path, Linux Git for a Windows folder), until dismissed for that tab. -->
      <div
        v-for="warning in repoWarnings"
        :key="warning"
        class="flex shrink-0 items-center gap-2 border-b border-modified/40 bg-modified/10 px-3 py-1 text-base text-modified-fg"
        role="status"
      >
        <TriangleAlertIcon class="size-3.5 shrink-0" />
        <span class="truncate" :title="warning">{{ sessions.active?.info.displayName }}: {{ warning }}</span>
        <button type="button" class="ml-auto rounded p-0.5 hover:bg-modified/20" aria-label="Dismiss" @click="dismissRepoWarning(warning)">
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
                <RepositoriesPanel @add-repo="dialogs.showAddRepo()" />
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
                      class="area"
                      message="Open a repository to get started."
                    >
                      <Button variant="outline" size="sm" @click="dialogs.showAddRepo()">
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
            <div v-if="consoleCollapsed" class="mt-1 shrink-0">
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
    <RenameBranchDialog />
    <CommitDialog />
    <StashDialog />
    <ApplyStashDialog />
    <PullDialog />
    <PushDialog />
    <ConfirmDialog />
    <SettingsDialog />
    <PassphraseDialog />
    <HostKeyDialog />
    <SshPasswordDialog />
    <AboutDialog />
  </TooltipProvider>
</template>
