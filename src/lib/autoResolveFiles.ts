/**
 * Resolving whole files whose conflicts are all simple (see `merge/autoResolve.ts`) as soon as
 * they appear, after whatever left them: a merge, rebase, cherry-pick, stash or a command typed
 * in the Console. The `merge.autoResolve` setting turns it off, asks first, or does it silently.
 */
import { watch } from "vue";
import { commands, type ConflictFile } from "@/lib/bindings";
import { errorMessage, IpcError, unwrap } from "@/lib/ipc";
import { resolvedFileText, resolveSimpleBlocks } from "@/lib/merge/autoResolve";
import { filesLabel, opFailureText, runOp } from "@/lib/repoActions";
import { useDialogsStore } from "@/stores/dialogs";
import { useSessionsStore, type RepoSession } from "@/stores/sessions";
import { useSettingsStore } from "@/stores/settings";

/** The conflicted paths already looked at, per repository; forgotten once it has none. */
const seen = new Map<string, Set<string>>();

/** Looks at every conflicted file that appears in any open repository. Call once, from the app's setup. */
export function watchForConflicts(): void {
  const sessions = useSessionsStore();
  watch(
    () => [...sessions.sessions.values()].map((s) => [s.key, conflictedPaths(s).join("\n")] as const),
    (now) => {
      const keys = new Set(now.map(([key]) => key));
      for (const key of seen.keys()) if (!keys.has(key)) seen.delete(key);
      for (const [key, joined] of now) {
        const paths = joined === "" ? [] : joined.split("\n");
        if (paths.length === 0) {
          seen.delete(key);
          continue;
        }
        const known = seen.get(key) ?? new Set<string>();
        seen.set(key, known);
        // A file put back with "Start Again from Git's Merge" stays seen, so it is not resolved again.
        const fresh = paths.filter((p) => !known.has(p));
        for (const p of fresh) known.add(p);
        if (fresh.length > 0) void resolveSimpleFiles(key, fresh);
      }
    },
  );
}

/** Conflicted files both sides still have, the ones with text to merge. */
function conflictedPaths(session: RepoSession): string[] {
  return (session.status?.entries ?? []).filter((e) => e.conflicted && ["UU", "AA"].includes(`${e.x}${e.y}`)).map((e) => e.path);
}

async function resolveSimpleFiles(key: string, paths: string[]): Promise<void> {
  const mode = useSettingsStore().settings?.merge?.autoResolve ?? "prompt";
  if (mode === "off") return;
  const sessions = useSessionsStore();
  const found: { path: string; file: ConflictFile; content: string }[] = [];
  for (const path of paths) {
    try {
      const file = await unwrap(commands.getConflictFile(key, path));
      if (file.refused !== null || file.blocks.length === 0) continue;
      const texts = resolveSimpleBlocks(file);
      if (texts.some((t) => t === null)) continue;
      found.push({ path, file, content: resolvedFileText(file, texts as string[]) });
    } catch {
      // A file that cannot be read is left for the user, as any other conflict.
    }
  }
  if (found.length === 0) return;
  const label = found.length === 1 ? found[0]!.path : `${found.length} files`;
  if (mode === "prompt") {
    const list = found.length === 1 ? "" : `\n\n${found.map((f) => f.path).join("\n")}`;
    const ok = await useDialogsStore().open("confirm", {
      title: "Resolve simple conflicts?",
      message: `Every conflict in ${label} is where both sides changed lines next to each other, without changing the same lines. Keep both sides' changes, save ${found.length === 1 ? "it" : "them"} and mark ${found.length === 1 ? "it" : "them"} resolved?${list}`,
      confirmLabel: "Resolve",
    });
    if (!ok) return;
  }
  const written: string[] = [];
  for (const f of found) {
    try {
      await unwrap(commands.writeWorktreeFile(key, f.path, f.content, f.file.hash));
      written.push(f.path);
    } catch (e) {
      // Changed since it was read (edited, or resolved another way): leave it.
      if (!(e instanceof IpcError && e.error.kind === "changedOnDisk")) sessions.notify(errorMessage(e));
    }
  }
  if (written.length === 0) return;
  try {
    const result = await runOp(key, () => commands.opStage(key, written));
    if (result.ok) sessions.notifySuccess(`Resolved ${filesLabel(written)} by keeping both sides' changes.`);
    else sessions.notify(opFailureText(result, `Marking ${filesLabel(written)} resolved`));
  } catch (e) {
    sessions.notify(errorMessage(e));
  }
}
