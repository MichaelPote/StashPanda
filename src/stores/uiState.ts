import { computed, ref, type WritableComputedRef } from "vue";
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

/**
 * Values the Console, Files panel and diff header kept in the WebView's localStorage before they
 * moved into the UI state (2026-09-30). Each turns the stored text into a patch, or null when the
 * text is not a value it knows. `useLocalStorage` stored strings as they are, numbers and booleans
 * as their text, and objects as JSON.
 */
const LEGACY_LOCAL_STORAGE: [string, (raw: string) => JsonObject | null][] = [
  [
    "stashpanda.console.filters",
    (raw) => {
      const v = JSON.parse(raw) as Record<string, unknown>;
      const out: JsonObject = {};
      for (const k of ["userAction", "internal", "watcher"]) if (typeof v[k] === "boolean") out[k] = v[k];
      return { console: out };
    },
  ],
  ["stashpanda.files.groupBy", (raw) => (["status", "folder", "none"].includes(raw) ? { files: { groupBy: raw } } : null)],
  ["stashpanda.files.sortBy", (raw) => (["name", "status", "modified"].includes(raw) ? { files: { sortBy: raw } } : null)],
  ["stashpanda.diff.mode", (raw) => (["unified", "side"].includes(raw) ? { diff: { layout: raw } } : null)],
  ["stashpanda.diff.context", (raw) => (Number(raw) > 0 ? { diff: { contextLines: Math.round(Number(raw)) } } : null)],
  ["stashpanda.diff.ignoreWhitespace", (raw) => ({ diff: { ignoreWhitespace: raw === "true" } })],
  ["stashpanda.diff.ignoreCrAtEol", (raw) => ({ diff: { ignoreCrAtEol: raw === "true" } })],
  ["stashpanda.diff.split", (raw) => (Number.isFinite(Number(raw)) ? { diff: { split: Number(raw) } } : null)],
];

/** Moves the old localStorage values into one patch and deletes them, so this runs only once. */
function takeLegacyLocalStorage(): JsonObject | null {
  let patch: JsonObject | null = null;
  for (const [key, toPatch] of LEGACY_LOCAL_STORAGE) {
    try {
      const raw = localStorage.getItem(key);
      if (raw === null) continue;
      const p = toPatch(raw);
      if (p) patch = patch ? combinePatches(patch, p) : p;
      localStorage.removeItem(key);
    } catch {
      // Unreadable storage or a malformed value: leave that one at its default.
    }
  }
  return patch;
}

export const useUiStateStore = defineStore("uiState", () => {
  const state = ref<UiState | null>(null);
  let listening = false;
  let inFlight = 0;

  let pending: JsonObject | null = null;
  let timer: ReturnType<typeof setTimeout> | null = null;

  async function load(): Promise<void> {
    state.value = await unwrap(commands.uiStateGet());
    const legacy = takeLegacyLocalStorage();
    if (legacy) await update(legacy);
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

  /**
   * One remembered value as a ref: reads it from the UI state with a default for a missing value,
   * and writes it with a debounced patch (shown at once, saved 300 ms after the last change).
   */
  function pref<T>(read: (s: UiState) => T | null | undefined, fallback: T, write: (value: T) => JsonObject): WritableComputedRef<T> {
    return computed({
      get: () => (state.value ? (read(state.value) ?? fallback) : fallback),
      set: (value: T) => patchDebounced(write(value)),
    });
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
    pref,
    panelSize,
    setPanelSize,
    setConsoleCollapsed,
    setSidebarCollapsed,
  };
});
