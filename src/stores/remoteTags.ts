import { computed, ref, type ComputedRef } from "vue";
import { defineStore } from "pinia";
import { commands } from "@/lib/bindings";
import { unwrap } from "@/lib/ipc";
import { defaultTagRemote } from "@/lib/repoActions";
import { tagSyncView, type RemoteTagAnswer, type TagSyncView } from "@/lib/tagSync";
import { useSessionsStore, type RepoSession } from "@/stores/sessions";
import { useSettingsStore } from "@/stores/settings";

/**
 * Each open repository's remotes' tags, from `git ls-remote --tags`, for marking which tags are
 * pushed. Asked once when a repository's graph first shows, and again after every fetch, pull and
 * push (`runOperation`), never on a plain refresh: each ask is a connection to every remote.
 */
export const useRemoteTagsStore = defineStore("remoteTags", () => {
  /** Repository key to remote name to its answer. A remote not in the map has not answered yet. */
  const answers = ref(new Map<string, Map<string, RemoteTagAnswer>>());
  const running = new Set<string>();
  /** Repositories asked for again while their remotes were being asked. */
  const again = new Set<string>();

  /** Asks every remote of repository `key` for its tags, replacing each answer as it comes. */
  async function refresh(key: string): Promise<void> {
    if (running.has(key)) {
      again.add(key);
      return;
    }
    const remotes = useSessionsStore().sessions.get(key)?.refs?.remotes.map((r) => r.name) ?? [];
    running.add(key);
    try {
      await Promise.all(
        remotes.map(async (remote) => {
          let answer: RemoteTagAnswer = null;
          try {
            const list = await unwrap(commands.queryRemoteTags(key, crypto.randomUUID(), remote));
            if (list.result.ok) answer = new Map(list.tags.map((t) => [t.name, t.tag.commit]));
          } catch {
            // Unreachable, or the repository closed: the tags are drawn as if unchecked.
          }
          const forRepo = new Map(answers.value.get(key) ?? []);
          forRepo.set(remote, answer);
          answers.value.set(key, forRepo);
        }),
      );
    } finally {
      running.delete(key);
    }
    if (again.delete(key)) await refresh(key);
  }

  /** Asks repository `key`'s remotes unless they have been asked already. */
  function ensure(key: string): void {
    if (!answers.value.has(key) && !running.has(key)) void refresh(key);
  }

  function forget(key: string): void {
    answers.value.delete(key);
  }

  return { answers, refresh, ensure, forget };
});

/**
 * The tag comparison for `session`, against every remote or only the default tag remote
 * (`appearance.compareTagsAllRemotes`). Asks the remotes the first time it is used.
 */
export function useTagSync(session: () => RepoSession | null | undefined): ComputedRef<TagSyncView | null> {
  const store = useRemoteTagsStore();
  const settings = useSettingsStore();
  return computed(() => {
    const s = session();
    if (!s?.refs) return null;
    store.ensure(s.key);
    const localTags = new Map<string, string>();
    for (const r of s.refs.refs) if (r.kind.type === "tag") localTags.set(r.short, r.target);
    const all = settings.settings?.appearance?.compareTagsAllRemotes ?? true;
    const fallback = defaultTagRemote(s);
    const compared = all ? s.refs.remotes.map((r) => r.name) : fallback ? [fallback] : [];
    return tagSyncView(localTags, store.answers.get(s.key) ?? new Map(), compared);
  });
}
