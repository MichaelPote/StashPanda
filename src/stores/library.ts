import { computed } from "vue";
import { defineStore } from "pinia";
import { commands, type CloneResult, type Connection, type LibraryGroup, type RepoEntry } from "@/lib/bindings";
import { unwrap } from "@/lib/ipc";
import { pathBasename } from "@/lib/format";
import { useSettingsStore } from "@/stores/settings";

export type LibraryNode =
  | { kind: "group"; id: string; group: LibraryGroup; children: LibraryNode[] }
  | { kind: "repo"; id: string; repo: RepoEntry };

export function repoDisplayName(repo: RepoEntry): string {
  if (repo.displayName && repo.displayName.length > 0) return repo.displayName;
  return repo.path ? pathBasename(repo.path) : (repo.key ?? "Repository");
}

function byName(a: string, b: string): number {
  return a.localeCompare(b, undefined, { sensitivity: "base" });
}

export const useLibraryStore = defineStore("library", () => {
  const settings = useSettingsStore();

  const groups = computed<LibraryGroup[]>(() => settings.settings?.library?.groups ?? []);
  const repos = computed<RepoEntry[]>(() => settings.settings?.library?.repos ?? []);

  const repoByKey = computed(() => {
    const map = new Map<string, RepoEntry>();
    for (const repo of repos.value) if (repo.key) map.set(repo.key, repo);
    return map;
  });

  /** Groups nested through `parentId`, repositories attached through `groupId`. */
  const tree = computed<LibraryNode[]>(() => {
    const groupIds = new Set(groups.value.map((g) => g.id).filter((id): id is string => !!id));

    function build(parentId: string | null): LibraryNode[] {
      const childGroups = groups.value
        .filter((g) => (g.parentId ?? null) === parentId && g.id)
        .sort((a, b) => byName(a.name ?? "", b.name ?? ""))
        .map<LibraryNode>((g) => ({ kind: "group", id: g.id as string, group: g, children: build(g.id as string) }));

      const childRepos = repos.value
        .filter((r) => {
          const gid = r.groupId ?? null;
          // Repositories pointing at a group that no longer exists fall back to the top level.
          const effective = gid !== null && groupIds.has(gid) ? gid : null;
          return effective === parentId && r.key;
        })
        .sort((a, b) => byName(repoDisplayName(a), repoDisplayName(b)))
        .map<LibraryNode>((r) => ({ kind: "repo", id: r.key as string, repo: r }));

      return [...childGroups, ...childRepos];
    }

    return build(null);
  });

  const isEmpty = computed(() => repos.value.length === 0 && groups.value.length === 0);

  async function addRepo(
    path: string,
    connection: Connection,
    groupId: string | null = null,
    displayName: string | null = null,
  ): Promise<RepoEntry> {
    const entry = await unwrap(commands.repoAdd(path, connection, groupId, displayName));
    await settings.refresh();
    return entry;
  }

  /**
   * Clones `url` into `path` and adds it to the library; see `repoClone`. Progress arrives as
   * `cloneProgress` events carrying `opId`, and `cloneCancel(opId)` stops it.
   */
  async function cloneRepo(
    opId: string,
    url: string,
    path: string,
    connection: Connection,
    displayName: string | null = null,
  ): Promise<CloneResult> {
    try {
      return await unwrap(commands.repoClone(opId, url, path, connection, null, displayName));
    } finally {
      await settings.refresh();
    }
  }

  /** Saves an edited entry. The returned entry has a new key when the path or connection moved it. */
  async function updateRepo(
    key: string,
    path: string,
    connection: Connection,
    displayName: string | null,
  ): Promise<RepoEntry> {
    const entry = await unwrap(commands.repoUpdate(key, path, connection, displayName));
    await settings.refresh();
    return entry;
  }

  /**
   * Gives a repository a colour (a hue, palette index 0 to 5), or takes it away with null: its
   * icon in the Repositories list, and the interface's accent while its tab is active. Saved in
   * the library, not in the repository.
   */
  async function setRepoColour(key: string, colour: number | null): Promise<void> {
    await unwrap(commands.repoSetColour(key, colour));
    await settings.refresh();
  }

  async function removeRepo(key: string): Promise<void> {
    await unwrap(commands.repoRemove(key));
    await settings.refresh();
  }

  /** Flips a group's `collapsed` flag. Arrays replace wholesale in a merge patch, so send them all. */
  async function setGroupCollapsed(groupId: string, collapsed: boolean): Promise<void> {
    const next = groups.value.map((g) => (g.id === groupId ? { ...g, collapsed } : g));
    await settings.update({ library: { groups: next } });
  }

  return { groups, repos, repoByKey, tree, isEmpty, addRepo, cloneRepo, updateRepo, setRepoColour, removeRepo, setGroupCollapsed };
});
