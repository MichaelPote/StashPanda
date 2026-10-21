<script setup lang="ts">
import { computed, reactive } from "vue";
import {
  ArchiveIcon,
  ChevronDownIcon,
  ChevronRightIcon,
  CloudIcon,
  FolderIcon,
  FolderOpenIcon,
  GitBranchIcon,
  TagIcon,
} from "@lucide/vue";
import type { RefEntry, StashEntry } from "@/lib/bindings";
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuSeparator,
  ContextMenuTrigger,
} from "@/components/ui/context-menu";
import { ScrollArea } from "@/components/ui/scroll-area";
import RefContextMenu from "@/components/graph/RefContextMenu.vue";
import AddBranchMenu from "@/components/shell/AddBranchMenu.vue";
import PanelHeader from "@/components/shell/PanelHeader.vue";
import RefToggleTag from "@/components/shell/RefToggleTag.vue";
import BranchName from "@/components/shell/BranchName.vue";
import { branchTreeRows } from "@/lib/branchTree";
import { formatRelativeTime } from "@/lib/format";
import { applyStash, dropStash, popStash, requestCheckout } from "@/lib/repoActions";
import { stashParts, stashTitle } from "@/lib/stash";
import { laneColour } from "@/lib/graph/paths";
import { cn } from "@/lib/utils";
import { useSessionsStore } from "@/stores/sessions";

const sessions = useSessionsStore();

const active = computed(() => sessions.active);
/** Key for the context menus; an empty string while no repository is open (the list is hidden then). */
const activeKey = computed(() => active.value?.key ?? "");
const refs = computed(() => active.value?.refs ?? null);

const byName = (a: RefEntry, b: RefEntry): number => a.short.localeCompare(b.short);

const locals = computed<RefEntry[]>(() =>
  (refs.value?.refs ?? []).filter((r) => r.kind.type === "localBranch").sort(byName),
);

const remoteGroups = computed<{ name: string; branches: RefEntry[] }[]>(() => {
  const groups = new Map<string, RefEntry[]>();
  for (const remote of refs.value?.remotes ?? []) groups.set(remote.name, []);
  for (const r of refs.value?.refs ?? []) {
    if (r.kind.type !== "remoteBranch") continue;
    let list = groups.get(r.kind.remote);
    if (!list) {
      list = [];
      groups.set(r.kind.remote, list);
    }
    list.push(r);
  }
  return [...groups.entries()]
    .map(([name, branches]) => ({ name, branches: branches.sort(byName) }))
    .sort((a, b) => a.name.localeCompare(b.name));
});

const tags = computed<RefEntry[]>(() =>
  (refs.value?.refs ?? [])
    .filter((r) => r.kind.type === "tag")
    .sort((a, b) => b.committerTime - a.committerTime || byName(a, b)),
);

const stashes = computed<StashEntry[]>(() => active.value?.stashes ?? []);
/** The list's length once loaded; until then the count from status. */
const stashCount = computed(() => active.value?.stashes?.length ?? active.value?.status?.stashCount ?? 0);


/** Collapsed groups, keyed by section or `remote:<name>`. Remotes start collapsed. */
const collapsed = reactive(new Set<string>());
const remotesInitialised = new Set<string>();

function isOpen(key: string): boolean {
  if (key.startsWith("remote:") && !remotesInitialised.has(key)) {
    remotesInitialised.add(key);
    collapsed.add(key);
  }
  return !collapsed.has(key);
}

function toggle(key: string): void {
  if (collapsed.has(key)) collapsed.delete(key);
  else collapsed.add(key);
}

/**
 * A heading's click, from the main button only. WebKitGTK (the Linux build) also sends a click
 * for a right-click, with `button` 0 but the right button still held in `buttons`, so opening a
 * heading's menu would fold it too.
 */
function onHeadingClick(event: MouseEvent, key: string): void {
  if (event.button === 0 && (event.buttons & 2) === 0) toggle(key);
}

/** Local branches in folders by the parts of their names (`feature/email` in `feature`). */
const localRows = computed(() => branchTreeRows(locals.value, (b) => b.short, "folder:local:", isOpen));

/** Each remote's branches in folders the same way, by their names on the remote. */
const remoteRows = computed(() =>
  remoteGroups.value.map((group) => ({ ...group, rows: branchTreeRows(group.branches, remoteShort, `folder:remote:${group.name}:`, isOpen) })),
);

/**
 * Indentation of a row `depth` folders deep below a heading indented `base` pixels. A level is
 * 20px, the width of a folder row's chevron and the gap after it plus a little: a folder's
 * branches then start under its name, and a folder inside it under its folder icon, so each level
 * reads as inside the one above.
 */
function indent(base: number, depth: number): Record<string, string> {
  return { paddingLeft: `${base + depth * 20}px` };
}

function remoteShort(entry: RefEntry): string {
  if (entry.kind.type !== "remoteBranch") return entry.short;
  const prefix = `${entry.kind.remote}/`;
  return entry.short.startsWith(prefix) ? entry.short.slice(prefix.length) : entry.short;
}

/** The branch a local branch tracks, as it is usually written (`origin/stage`), or null. */
function upstreamName(entry: RefEntry): string | null {
  const up = entry.upstream;
  if (!up) return null;
  return up.replace(/^refs\/(remotes|heads)\//, "");
}

/** Double-clicking a branch checks it out, with the same checks as the Checkout menu item. */
function checkoutOnDoubleClick(entry: RefEntry): void {
  const session = active.value;
  if (!session || entry.isHead) return;
  if (entry.kind.type === "localBranch") void requestCheckout(session.key, { kind: "local", name: entry.short });
  else if (entry.kind.type === "remoteBranch") {
    void requestCheckout(session.key, { kind: "remote", remote: entry.short, local: remoteShort(entry) });
  }
}

const selectedOid = computed(() => active.value?.selectedRow?.oid ?? null);

/** Lane colour of a ref's tip in the current graph, or null when it is not in the graph. */
function colourOf(entry: RefEntry): number | null {
  return active.value?.graph.meta?.refColours?.[entry.fullName] ?? null;
}

function isShown(entry: RefEntry): boolean {
  return !(active.value?.hiddenRefs.has(entry.fullName) ?? false);
}

function toggleShown(entry: RefEntry): void {
  const session = active.value;
  if (session) void sessions.toggleRefVisible(session.key, entry.fullName);
}

async function reveal(entry: RefEntry): Promise<void> {
  const session = active.value;
  if (!session) return;
  const found = await sessions.selectCommit(session.key, entry.target);
  if (!found) {
    sessions.notify(
      isShown(entry)
        ? `${entry.short} points at a commit that is not in the loaded graph.`
        : `${entry.short} is hidden in the graph. Click its coloured tag to show it again.`,
    );
  }
}
</script>

<template>
  <section class="area flex h-full min-h-0 flex-col">
    <PanelHeader title="Branches">
      <span v-if="active" class="truncate text-xs text-muted-foreground">
        {{ active.info.displayName }}
      </span>
    </PanelHeader>

    <div v-if="!active" class="flex min-h-0 flex-1 items-center justify-center text-base text-muted-foreground">
      No repository open
    </div>

    <ScrollArea v-else class="min-h-0 flex-1">
      <div class="py-1 text-base select-none">
        <!-- Local branches, in folders by the parts of their names. -->
        <AddBranchMenu :session-key="activeKey">
          <button type="button" class="flex h-(--row-h) w-full items-center gap-1.5 px-2 smallcaps hover:text-foreground" @click="onHeadingClick($event, 'local')">
            <component :is="isOpen('local') ? ChevronDownIcon : ChevronRightIcon" class="size-3" />
            <GitBranchIcon class="size-3.5" />
            Local Branches
            <span class="ml-auto font-medium tracking-normal normal-case tabular-nums">{{ locals.length }}</span>
          </button>
        </AddBranchMenu>
        <template v-if="isOpen('local')">
          <p v-if="locals.length === 0" class="h-(--row-h) px-7 leading-(--row-h) text-muted-foreground italic">No local branches</p>
          <template v-for="row in localRows" :key="row.key">
            <AddBranchMenu v-if="row.kind === 'folder'" :session-key="activeKey" :prefix="row.prefix">
              <button
                type="button"
                class="flex h-(--row-h) w-full items-center gap-1.5 pr-2 text-left hover:bg-row-hover"
                :style="indent(12, row.depth)"
                :title="row.prefix"
                data-branch-folder
                @click="onHeadingClick($event, row.key)"
              >
                <component :is="isOpen(row.key) ? ChevronDownIcon : ChevronRightIcon" class="size-3 shrink-0 text-muted-foreground" />
                <component :is="isOpen(row.key) ? FolderOpenIcon : FolderIcon" class="size-3.5 shrink-0 text-muted-foreground" />
                <span class="truncate">{{ row.name }}</span>
                <span class="ml-auto text-[0.7rem] text-muted-foreground tabular-nums">{{ row.count }}</span>
              </button>
            </AddBranchMenu>
            <RefContextMenu
              v-else
              :session-key="activeKey"
              :full-name="row.entry.fullName"
              :short="row.entry.short"
              :kind="row.entry.kind"
              :target="row.entry.target"
              :is-current="row.entry.isHead"
            >
              <div
                role="button"
                tabindex="0"
                :class="cn('flex h-(--row-h) w-full items-center gap-1.5 pr-2 text-left', selectedOid === row.entry.target ? 'row-selected' : 'hover:bg-row-hover')"
                :style="indent(12, row.depth)"
                :title="`${row.entry.fullName}\n${row.entry.target}\n${row.entry.subject}`"
                @click="reveal(row.entry)"
                @dblclick="checkoutOnDoubleClick(row.entry)"
                @keydown.enter="reveal(row.entry)"
              >
                <!-- The checked-out branch: the graph's HEAD dot (GraphLanesSvg.vue), same size and colour. -->
                <svg
                  v-if="row.entry.isHead"
                  viewBox="0 0 12 12"
                  class="size-[12px] shrink-0 text-muted-foreground"
                  :style="colourOf(row.entry) !== null ? { color: laneColour(colourOf(row.entry)!) } : undefined"
                  role="img"
                  aria-label="Checked out"
                >
                  <circle cx="6" cy="6" r="5" fill="none" stroke="currentColor" stroke-width="2" />
                  <circle cx="6" cy="6" r="2" fill="currentColor" />
                </svg>
                <span v-else class="size-[12px] shrink-0" />
                <RefToggleTag :colour="colourOf(row.entry)" :visible="isShown(row.entry)" :name="row.entry.short" @toggle="toggleShown(row.entry)" />
                <BranchName :name="row.entry.short" :class="cn(row.entry.isHead && 'font-semibold', !isShown(row.entry) && 'text-muted-foreground')" />
                <!-- The tracked branch gives way first when the row is narrow. -->
                <span
                  v-if="upstreamName(row.entry)"
                  class="flex min-w-0 shrink-[4] items-center gap-0.5 text-muted-foreground"
                  :title="`Tracks ${upstreamName(row.entry)}`"
                >
                  <ChevronRightIcon class="size-3 shrink-0" aria-hidden="true" />
                  <span class="min-w-0 truncate">{{ upstreamName(row.entry) }}</span>
                </span>
                <span class="ml-auto flex shrink-0 items-center gap-1 text-[0.7rem] tabular-nums">
                  <span v-if="row.entry.upstreamGone" class="rounded-sm badge-deleted px-1">gone</span>
                  <span v-if="row.entry.ahead" class="rounded-sm badge-modified px-1">↑{{ row.entry.ahead }}</span>
                  <span v-if="row.entry.behind" class="rounded-sm badge-primary px-1">↓{{ row.entry.behind }}</span>
                </span>
              </div>
            </RefContextMenu>
          </template>
        </template>

        <!-- Remotes, each with its branches in folders by the parts of their names. -->
        <AddBranchMenu :session-key="activeKey">
          <button type="button" class="mt-1 flex h-(--row-h) w-full items-center gap-1.5 px-2 smallcaps hover:text-foreground" @click="onHeadingClick($event, 'remotes')">
            <component :is="isOpen('remotes') ? ChevronDownIcon : ChevronRightIcon" class="size-3" />
            <CloudIcon class="size-3.5" />
            Remotes
            <span class="ml-auto font-medium tracking-normal normal-case tabular-nums">{{ remoteGroups.length }}</span>
          </button>
        </AddBranchMenu>
        <template v-if="isOpen('remotes')">
          <p v-if="remoteGroups.length === 0" class="h-(--row-h) px-7 leading-(--row-h) text-muted-foreground italic">No remotes</p>
          <template v-for="group in remoteRows" :key="group.name">
            <AddBranchMenu :session-key="activeKey">
              <button type="button" class="flex h-(--row-h) w-full items-center gap-1.5 pr-2 pl-5 text-left hover:bg-row-hover" @click="onHeadingClick($event, `remote:${group.name}`)">
                <component :is="isOpen(`remote:${group.name}`) ? ChevronDownIcon : ChevronRightIcon" class="size-3 text-muted-foreground" />
                <span class="truncate font-medium">{{ group.name }}</span>
                <span class="ml-auto text-[0.7rem] text-muted-foreground tabular-nums">{{ group.branches.length }}</span>
              </button>
            </AddBranchMenu>
            <template v-if="isOpen(`remote:${group.name}`)">
              <template v-for="row in group.rows" :key="row.key">
                <AddBranchMenu v-if="row.kind === 'folder'" :session-key="activeKey" :prefix="row.prefix">
                  <button
                    type="button"
                    class="flex h-(--row-h) w-full items-center gap-1.5 pr-2 text-left hover:bg-row-hover"
                    :style="indent(32, row.depth)"
                    :title="`${group.name}/${row.prefix}`"
                    data-branch-folder
                    @click="onHeadingClick($event, row.key)"
                  >
                    <component :is="isOpen(row.key) ? ChevronDownIcon : ChevronRightIcon" class="size-3 shrink-0 text-muted-foreground" />
                    <component :is="isOpen(row.key) ? FolderOpenIcon : FolderIcon" class="size-3.5 shrink-0 text-muted-foreground" />
                    <span class="truncate">{{ row.name }}</span>
                    <span class="ml-auto text-[0.7rem] text-muted-foreground tabular-nums">{{ row.count }}</span>
                  </button>
                </AddBranchMenu>
                <RefContextMenu
                  v-else
                  :session-key="activeKey"
                  :full-name="row.entry.fullName"
                  :short="row.entry.short"
                  :kind="row.entry.kind"
                  :target="row.entry.target"
                  :is-current="false"
                >
                  <div
                    role="button"
                    tabindex="0"
                    :class="cn('flex h-(--row-h) w-full items-center gap-1.5 pr-2 text-left', selectedOid === row.entry.target ? 'row-selected' : 'hover:bg-row-hover')"
                    :style="indent(32, row.depth)"
                    :title="`${row.entry.fullName}\n${row.entry.target}\n${row.entry.subject}`"
                    @click="reveal(row.entry)"
                    @dblclick="checkoutOnDoubleClick(row.entry)"
                    @keydown.enter="reveal(row.entry)"
                  >
                    <RefToggleTag :colour="colourOf(row.entry)" :visible="isShown(row.entry)" :name="row.entry.short" @toggle="toggleShown(row.entry)" />
                    <BranchName :name="remoteShort(row.entry)" class="text-muted-foreground" />
                  </div>
                </RefContextMenu>
              </template>
            </template>
          </template>
        </template>

        <!-- Tags -->
        <AddBranchMenu :session-key="activeKey">
          <button type="button" class="mt-1 flex h-(--row-h) w-full items-center gap-1.5 px-2 smallcaps hover:text-foreground" @click="onHeadingClick($event, 'tags')">
            <component :is="isOpen('tags') ? ChevronDownIcon : ChevronRightIcon" class="size-3" />
            <TagIcon class="size-3.5" />
            Tags
            <span class="ml-auto font-medium tracking-normal normal-case tabular-nums">{{ tags.length }}</span>
          </button>
        </AddBranchMenu>
        <template v-if="isOpen('tags')">
          <p v-if="tags.length === 0" class="h-(--row-h) px-7 leading-(--row-h) text-muted-foreground italic">No tags</p>
          <RefContextMenu
            v-for="t in tags"
            :key="t.fullName"
            :session-key="activeKey"
            :full-name="t.fullName"
            :short="t.short"
            :kind="t.kind"
            :target="t.target"
            :is-current="false"
          >
            <div
              role="button"
              tabindex="0"
              :class="cn('flex h-(--row-h) w-full items-center gap-1.5 pr-2 pl-3 text-left', selectedOid === t.target ? 'row-selected' : 'hover:bg-row-hover')"
              :title="`${t.fullName}\n${t.target}\n${t.subject}`"
              @click="reveal(t)"
              @keydown.enter="reveal(t)"
            >
              <span class="size-3.5 shrink-0" />
              <RefToggleTag :colour="colourOf(t)" :visible="isShown(t)" :name="t.short" @toggle="toggleShown(t)" />
              <span :class="cn('truncate', !isShown(t) && 'text-muted-foreground')">{{ t.short }}</span>
              <span v-if="t.kind.type === 'tag' && t.kind.annotated" class="ml-auto text-[0.7rem] text-muted-foreground">annotated</span>
            </div>
          </RefContextMenu>
        </template>

        <!-- Stashes -->
        <AddBranchMenu :session-key="activeKey">
          <button type="button" class="mt-1 flex h-(--row-h) w-full items-center gap-1.5 px-2 smallcaps hover:text-foreground" @click="onHeadingClick($event, 'stashes')">
            <component :is="isOpen('stashes') ? ChevronDownIcon : ChevronRightIcon" class="size-3" />
            <ArchiveIcon class="size-3.5" />
            Stashes
            <span class="ml-auto font-medium tracking-normal normal-case tabular-nums">{{ stashCount }}</span>
          </button>
        </AddBranchMenu>
        <template v-if="isOpen('stashes')">
          <p v-if="stashCount === 0" class="h-(--row-h) px-7 leading-(--row-h) text-muted-foreground italic">No stashes</p>
          <!-- Stash commits are not in the graph, so a click has nothing to reveal; the actions are
               in the context menu. -->
          <ContextMenu v-for="s in stashes" :key="s.oid">
            <ContextMenuTrigger as-child>
              <div class="flex h-(--row-h) w-full items-center gap-1.5 pr-2 pl-3 hover:bg-row-hover" :title="stashTitle(s)">
                <span class="size-3.5 shrink-0" />
                <span class="truncate">{{ stashParts(s).text }}</span>
                <span class="ml-auto shrink-0 text-[0.7rem] text-muted-foreground tabular-nums">{{ formatRelativeTime(s.time) }}</span>
              </div>
            </ContextMenuTrigger>
            <ContextMenuContent class="min-w-52">
              <ContextMenuItem class="text-base" @select="applyStash(activeKey, s)">Apply (keep the stash)</ContextMenuItem>
              <ContextMenuItem class="text-base" @select="popStash(activeKey, s)">Pop (apply, then drop)</ContextMenuItem>
              <ContextMenuSeparator />
              <ContextMenuItem class="text-base" variant="destructive" @select="dropStash(activeKey, s, stashParts(s).text)">
                Drop…
              </ContextMenuItem>
            </ContextMenuContent>
          </ContextMenu>
        </template>
      </div>
    </ScrollArea>
  </section>
</template>
