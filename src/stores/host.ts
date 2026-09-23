import { ref } from "vue";
import { defineStore } from "pinia";
import { commands, events, type HostInfo } from "@/lib/bindings";
import { unwrap } from "@/lib/ipc";

/** Host operating system plus the non-fatal warnings the backend wants shown. */
export const useHostStore = defineStore("host", () => {
  const info = ref<HostInfo | null>(null);
  const warnings = ref<string[]>([]);
  let listening = false;

  async function load(): Promise<void> {
    const [host, startup] = await Promise.all([
      unwrap(commands.hostInfo()),
      unwrap(commands.startupWarnings()),
    ]);
    info.value = host;
    warnings.value = startup;
    if (!listening) {
      listening = true;
      await events.appWarning.listen((event) => {
        warnings.value = [...warnings.value, event.payload.message];
      });
    }
  }

  function dismissWarning(index: number): void {
    warnings.value = warnings.value.filter((_, i) => i !== index);
  }

  return { info, warnings, load, dismissWarning };
});
