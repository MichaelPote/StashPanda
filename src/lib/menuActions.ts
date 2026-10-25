/**
 * The one listener for the native menu bar. The Rust side (`src-tauri/src/menu.rs`) emits
 * `menu-action-event` with a dotted id; `dispatch` maps each id to a store call or a dialog.
 */
import type { UnlistenFn } from "@tauri-apps/api/event";
import { openUrl } from "@tauri-apps/plugin-opener";
import { events, type Theme } from "@/lib/bindings";
import { errorMessage } from "@/lib/ipc";
import { shortSha } from "@/lib/refNames";
import {
  createBranchHere,
  createTagAt,
  fetchAll,
  openApplyStashDialog,
  openMergeDialog,
  openPullDialog,
  openRebaseDialog,
  openRemoteDialog,
  openStashDialog,
  pushCurrentBranch,
  removeFromLibrary,
  syncCurrentBranch,
} from "@/lib/repoActions";
import { useDialogsStore } from "@/stores/dialogs";
import { useSessionsStore, type RepoSession } from "@/stores/sessions";
import { useSettingsStore } from "@/stores/settings";
import { useUiStateStore } from "@/stores/uiState";

const DOCS_URL = "https://github.com/";

/** The active session, or null after posting a notice that says to open a repository first. */
function requireActive(): RepoSession | null {
  const sessions = useSessionsStore();
  const session = sessions.active;
  if (!session) sessions.notify("Open a repository first.");
  return session;
}

/** HEAD's oid plus a label (branch name or short sha) for the Create Branch and Create Tag dialogs. */
function headOf(session: RepoSession): { oid: string; label: string } | null {
  const oid = session.status?.headOid ?? session.refs?.head ?? session.graph.meta?.head ?? null;
  if (oid === null) return null;
  return { oid, label: session.status?.branch ?? shortSha(oid) };
}

async function openDocs(): Promise<void> {
  const sessions = useSessionsStore();
  try {
    await openUrl(DOCS_URL);
  } catch (e) {
    sessions.notify(`Could not open ${DOCS_URL}: ${errorMessage(e)}`);
  }
}

export async function dispatch(id: string): Promise<void> {
  const sessions = useSessionsStore();
  const uiState = useUiStateStore();
  const settings = useSettingsStore();
  const dialogs = useDialogsStore();

  switch (id) {
    case "file.open":
      dialogs.showAddRepo();
      return;
    case "file.clone":
      dialogs.cloneRepo();
      return;
    case "file.close_tab":
      if (sessions.activeKey !== null) await sessions.closeTab(sessions.activeKey);
      return;
    case "file.settings":
      await dialogs.open("settings", {});
      return;
    case "edit.find":
      sessions.openFind();
      return;
    case "view.refresh":
    case "repo.refresh":
      if (sessions.activeKey !== null) await sessions.refreshSession(sessions.activeKey);
      return;
    case "view.toggle_console":
      uiState.setConsoleCollapsed(!uiState.consoleCollapsed);
      return;
    case "view.toggle_sidebar":
      uiState.setSidebarCollapsed(!uiState.sidebarCollapsed);
      return;
    case "view.theme.dark":
    case "view.theme.light":
    case "view.theme.system": {
      const theme = id.slice("view.theme.".length) as Theme;
      try {
        await settings.update({ appearance: { theme } });
      } catch (e) {
        sessions.notify(errorMessage(e));
      }
      return;
    }
    case "repo.create_branch": {
      const session = requireActive();
      if (!session) return;
      const head = headOf(session);
      if (!head) {
        sessions.notify("This repository has no commits yet, so there is nothing to branch from.");
        return;
      }
      await createBranchHere(session.key, head.oid, head.label);
      return;
    }
    case "repo.create_tag": {
      const session = requireActive();
      if (!session) return;
      const head = headOf(session);
      if (!head) {
        sessions.notify("This repository has no commits yet, so there is nothing to tag.");
        return;
      }
      await createTagAt(session.key, head.oid, head.label);
      return;
    }
    case "repo.stash":
      await openStashDialog(sessions.active);
      return;
    case "repo.apply_stash":
      await openApplyStashDialog(sessions.active);
      return;
    case "repo.pull":
      await openPullDialog(sessions.active);
      return;
    case "repo.fetch":
      await fetchAll(sessions.active);
      return;
    case "repo.push":
      await pushCurrentBranch(sessions.active);
      return;
    case "repo.merge":
      await openMergeDialog(sessions.active);
      return;
    case "repo.rebase":
      await openRebaseDialog(sessions.active);
      return;
    case "repo.sync":
      await syncCurrentBranch(sessions.active);
      return;
    case "repo.remote_add":
      await openRemoteDialog(sessions.active, "add");
      return;
    case "repo.remote_edit":
      await openRemoteDialog(sessions.active, "edit");
      return;
    case "repo.remote_rename":
      await openRemoteDialog(sessions.active, "rename");
      return;
    case "repo.remote_remove":
      await openRemoteDialog(sessions.active, "remove");
      return;
    case "repo.remove": {
      // The same removal as the Repositories context menu and the Edit Repository dialog.
      const session = requireActive();
      if (session) await removeFromLibrary(session.key);
      return;
    }
    case "help.docs":
      await openDocs();
      return;
    case "help.console_log":
      uiState.setConsoleCollapsed(false);
      return;
    case "help.about":
      await dialogs.open("about", null);
      return;
    default:
      console.warn(`Unhandled menu action: ${id}`);
  }
}

/** Subscribes to `menu-action-event`. Call once from `App.vue`; the returned function unsubscribes. */
export function installMenuActions(): Promise<UnlistenFn> {
  return events.menuActionEvent.listen((event) => {
    void dispatch(event.payload.id);
  });
}
