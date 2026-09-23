import { computed, ref } from "vue";
import { defineStore } from "pinia";
import { commands, events, type Panels, type UiState } from "@/lib/bindings";
import { unwrap } from "@/lib/ipc";
import { applyMergePatch, combinePatches, type JsonObject } from "@/lib/mergePatch";

/** Percentages used when the saved UI state has no value yet. */
export const PANEL_DEFAULTS = {
  leftWidth: 22,
  reposHeight: 50,
  filesWidth: 40,
  diffHeight: 40,
  consoleHeight: 25,
} as const;

type PanelSizeKey = keyof typeof PANEL_DEFAULTS;

const DEBOUNCE_MS = 300;

export const useUiStateStore = defineStore("uiState", () => {
  const state = ref<UiState | null>(null);
  let listening = false;
  let inFlight = 0;

  let pending: JsonObject | null = null;
  let timer: ReturnType<typeof setTimeout> | null = null;

  async function load(): Promise<void> {
    state.value = await unwrap(commands.uiStateGet());
    if (!listening) {
      listening = true;
      await events.uiStateChanged.listen(async () => {
        // The event only carries a revision, so re-fetch. Skip while one of our own
        // patches is in flight: its response already carries the newest state.
        if (inFlight > 0) return;
        state.value = await unwrap(commands.uiStateGet());
      });
    }
  }

  /** Applies a patch locally at once and sends it to the backend. */
  async function update(patch: JsonObject): Promise<void> {
    state.value = applyMergePatch(state.value ?? {}, patch);
    inFlight += 1;
    try {
      state.value = await unwrap(commands.uiStateUpdate(JSON.stringify(patch)));
    } finally {
      inFlight -= 1;
    }
  }

  /** Sends all pending debounced patches now. */
  async function flush(): Promise<void> {
    if (timer !== null) {
      clearTimeout(timer);
      timer = null;
    }
    const patch = pending;
    pending = null;
    if (patch) await update(patch);
  }

  /** Merges patches for 300 ms before sending, for splitter drags and tab changes. */
  function patchDebounced(patch: JsonObject): void {
    state.value = applyMergePatch(state.value ?? {}, patch);
    pending = pending ? combinePatches(pending, patch) : patch;
    if (timer !== null) clearTimeout(timer);
    timer = setTimeout(() => {
      timer = null;
      void flush();
    }, DEBOUNCE_MS);
  }

  const panels = computed<Panels>(() => state.value?.panels ?? {});

  /** Saved percentage for a splitter panel, or the default when nothing is saved. */
  function panelSize(key: PanelSizeKey): number {
    const saved = panels.value[key];
    return typeof saved === "number" && Number.isFinite(saved) ? saved : PANEL_DEFAULTS[key];
  }

  /** Persists a splitter size, skipping writes that would not change the saved value. */
  function setPanelSize(key: PanelSizeKey, size: number): void {
    const rounded = Math.round(size * 100) / 100;
    if (Math.abs(panelSize(key) - rounded) < 0.01) return;
    patchDebounced({ panels: { [key]: rounded } });
  }

  const consoleCollapsed = computed(() => panels.value.consoleCollapsed ?? false);

  function setConsoleCollapsed(collapsed: boolean): void {
    void update({ panels: { consoleCollapsed: collapsed } });
  }

  /** Hides the left sidebar (Repositories and Branches). Session-only: it is not written to disk. */
  const sidebarCollapsed = ref(false);

  function setSidebarCollapsed(collapsed: boolean): void {
    sidebarCollapsed.value = collapsed;
  }

  return {
    state,
    panels,
    consoleCollapsed,
    sidebarCollapsed,
    load,
    update,
    flush,
    patchDebounced,
    panelSize,
    setPanelSize,
    setConsoleCollapsed,
    setSidebarCollapsed,
  };
});
