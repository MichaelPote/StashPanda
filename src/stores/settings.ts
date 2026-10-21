import { computed, ref } from "vue";
import { defineStore } from "pinia";
import { commands, events, type Settings } from "@/lib/bindings";
import { unwrap } from "@/lib/ipc";
import type { JsonObject } from "@/lib/mergePatch";

export const useSettingsStore = defineStore("settings", () => {
  const settings = ref<Settings | null>(null);
  /**
   * The toolbar layout being edited on the Settings dialog's Toolbar page, before Save Changes:
   * the toolbar shows it instead of the saved one while it is set, and the dialog clears it when
   * it closes, so Cancel puts the toolbar back. Null when nothing is being edited.
   */
  const toolbarDraft = ref<string[] | null>(null);
  let listening = false;

  /** Fetches settings from the backend and subscribes to `settings-changed` once. */
  async function load(): Promise<void> {
    settings.value = await unwrap(commands.settingsGet());
    if (!listening) {
      listening = true;
      await events.settingsChanged.listen((event) => {
        settings.value = event.payload.settings;
      });
    }
  }

  /** Re-fetches without touching the event subscription (used after library changes). */
  async function refresh(): Promise<void> {
    settings.value = await unwrap(commands.settingsGet());
  }

  /** Sends an RFC 7396 merge patch and replaces the local copy with the result. */
  async function update(patch: JsonObject): Promise<void> {
    settings.value = await unwrap(commands.settingsUpdate(JSON.stringify(patch)));
  }

  const consoleMaxLines = computed(() => settings.value?.console?.maxLines ?? 2000);
  const theme = computed(() => settings.value?.appearance?.theme ?? "dark");

  return { settings, toolbarDraft, load, refresh, update, consoleMaxLines, theme };
});
