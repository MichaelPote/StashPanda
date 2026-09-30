import { commands } from "@/lib/bindings";
import { unwrap } from "@/lib/ipc";

let monospace: Promise<string[]> | null = null;

/**
 * The monospaced font families installed on this machine. The scan reads every installed font,
 * so it runs once per app run; a failed scan is forgotten so the next call tries again.
 */
export function installedMonospaceFonts(): Promise<string[]> {
  monospace ??= unwrap(commands.fontsMonospace()).catch((e: unknown) => {
    monospace = null;
    throw e;
  });
  return monospace;
}
