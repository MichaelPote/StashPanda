/**
 * New versions of StashPanda, from the GitHub releases. The updater plugin reads `latest.json`
 * from the newest published release (`plugins.updater` in `src-tauri/tauri.conf.json`), which the
 * release job in `.github/workflows/build.yml` attaches; a draft release is not seen until it is
 * published. `checkForUpdates` looks for one and opens the Update dialog, which installs it.
 */
import { check } from "@tauri-apps/plugin-updater";
import { errorMessage } from "@/lib/ipc";
import { useDialogsStore } from "@/stores/dialogs";
import { useSessionsStore } from "@/stores/sessions";

export const RELEASES_URL = "https://github.com/MichaelPote/StashPanda/releases";

/** Set while a check is running, so a second request does not start another. */
let checking = false;

/**
 * Looks for a newer version and offers it in the Update dialog. `quiet` is the check when the app
 * starts: it says nothing when there is no update or the check fails (offline, say), and when a
 * dialog is already open it posts a notice with an Install button rather than replacing that
 * dialog. Help > Check for Updates passes false, and is told the outcome either way.
 */
export async function checkForUpdates(quiet: boolean): Promise<void> {
  if (checking) return;
  checking = true;
  const sessions = useSessionsStore();
  const dialogs = useDialogsStore();
  try {
    const update = await check();
    if (update === null) {
      if (!quiet) sessions.notifySuccess("StashPanda is up to date.");
      return;
    }
    if (quiet && (dialogs.active !== null || dialogs.overlay !== null)) {
      sessions.notify(`StashPanda ${update.version} is available.`, {
        label: "Install…",
        run: () => void dialogs.open("update", { update }),
      });
      return;
    }
    await dialogs.open("update", { update });
  } catch (e) {
    if (quiet) console.warn("Could not check for updates:", e);
    else sessions.notify(`Could not check for updates: ${errorMessage(e)}`);
  } finally {
    checking = false;
  }
}
