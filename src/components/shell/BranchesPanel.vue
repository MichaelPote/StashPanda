<script setup lang="ts">
import { computed, reactive } from "vue";
import {
  ArchiveIcon,
  ChevronDownIcon,
  ChevronRightIcon,
  CloudIcon,
  GitBranchIcon,
  TagIcon,
} from "@lucide/vue";
import type { RefEntry } from "@/lib/bindings";
import { ScrollArea } from "@/components/ui/scroll-area";
import RefContextMenu from "@/components/graph/RefContextMenu.vue";
import PanelHeader from "@/components/shell/PanelHeader.vue";
import RefToggleTag from "@/components/shell/RefToggleTag.vue";
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

const stashCount = computed(() => active.value?.status?.stashCount ?? 0);

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

function remoteShort(entry: RefEntry): string {
  if (entry.kind.type !== "remoteBranch") return entry.short;
  const prefix = `${entry.kind.remote}/`;
  return entry.short.startsWith(prefix) ? entry.short.slice(prefix.length) : entry.short;
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
        <!-- Local branches -->
        <button type="button" class="flex h-(--row-h) w-full cursor-pointer items-center gap-1.5 px-2 smallcaps hover:text-foreground" @click="toggle('local')">
          <component :is="isOpen('local') ? ChevronDownIcon : ChevronRightIcon" class="size-3" />
          <GitBranchIcon class="size-3.5" />
          Local Branches
          <span class="ml-auto font-medium tracking-normal normal-case tabular-nums">{{ locals.length }}</span>
        </button>
        <template v-if="isOpen('local')">
          <p v-if="locals.length === 0" class="h-(--row-h) px-7 leading-(--row-h) text-muted-foreground italic">No local branches</p>
          <RefContextMenu
            v-for="b in locals"
            :key="b.fullName"
            :session-key="activeKey"
            :full-name="b.fullName"
            :short="b.short"
            :kind="b.kind"
            :target="b.target"
            :is-current="b.isHead"
          >
            <div
              role="button"
              tabindex="0"
              :class="cn('flex h-(--row-h) w-full cursor-pointer items-center gap-1.5 pr-2 pl-3 text-left', selectedOid === b.target ? 'row-selected' : 'hover:bg-row-hover')"
              :title="`${b.fullName}\n${b.target}\n${b.subject}`"
              @click="reveal(b)"
              @keydown.enter="reveal(b)"
            >
              <ChevronRightIcon v-if="b.isHead" class="size-3.5 shrink-0 text-primary-tint" aria-label="Checked out" />
              <span v-else class="size-3.5 shrink-0" />
              <RefToggleTag :colour="colourOf(b)" :visible="isShown(b)" :name="b.short" @toggle="toggleShown(b)" />
              <span :class="cn('truncate', b.isHead && 'font-semibold', !isShown(b) && 'text-muted-foreground')">{{ b.short }}</span>
              <span class="ml-auto flex shrink-0 items-center gap-1 text-[0.7rem] tabular-nums">
                <span v-if="b.upstreamGone" class="rounded-sm badge-deleted px-1">gone</span>
                <span v-if="b.ahead" class="rounded-sm badge-modified px-1">↑{{ b.ahead }}</span>
                <span v-if="b.behind" class="rounded-sm badge-primary px-1">↓{{ b.behind }}</span>
              </span>
            </div>
          </RefContextMenu>
        </template>

        <!-- Remotes -->
        <button type="button" class="mt-1 flex h-(--row-h) w-full cursor-pointer items-center gap-1.5 px-2 smallcaps hover:text-foreground" @click="toggle('remotes')">
          <component :is="isOpen('remotes') ? ChevronDownIcon : ChevronRightIcon" class="size-3" />
          <CloudIcon class="size-3.5" />
          Remotes
          <span class="ml-auto font-medium tracking-normal normal-case tabular-nums">{{ remoteGroups.length }}</span>
        </button>
        <template v-if="isOpen('remotes')">
          <p v-if="remoteGroups.length === 0" class="h-(--row-h) px-7 leading-(--row-h) text-muted-foreground italic">No remotes</p>
          <template v-for="group in remoteGroups" :key="group.name">
            <button type="button" class="flex h-(--row-h) w-full cursor-pointer items-center gap-1.5 pr-2 pl-5 text-left hover:bg-row-hover" @click="toggle(`remote:${group.name}`)">
              <component :is="isOpen(`remote:${group.name}`) ? ChevronDownIcon : ChevronRightIcon" class="size-3 text-muted-foreground" />
              <span class="truncate font-medium">{{ group.name }}</span>
              <span class="ml-auto text-[0.7rem] text-muted-foreground tabular-nums">{{ group.branches.length }}</span>
            </button>
            <template v-if="isOpen(`remote:${group.name}`)">
              <RefContextMenu
                v-for="b in group.branches"
                :key="b.fullName"
                :session-key="activeKey"
                :full-name="b.fullName"
                :short="b.short"
                :kind="b.kind"
                :target="b.target"
                :is-current="false"
              >
                <div
                  role="button"
                  tabindex="0"
                  :class="cn('flex h-(--row-h) w-full cursor-pointer items-center gap-1.5 pr-2 pl-8 text-left', selectedOid === b.target ? 'row-selected' : 'hover:bg-row-hover')"
                  :title="`${b.fullName}\n${b.target}\n${b.subject}`"
                  @click="reveal(b)"
                  @keydown.enter="reveal(b)"
                >
                  <RefToggleTag :colour="colourOf(b)" :visible="isShown(b)" :name="b.short" @toggle="toggleShown(b)" />
                  <span class="truncate text-muted-foreground">{{ remoteShort(b) }}</span>
                </div>
              </RefContextMenu>
            </template>
          </template>
        </template>

        <!-- Tags -->
        <button type="button" class="mt-1 flex h-(--row-h) w-full cursor-pointer items-center gap-1.5 px-2 smallcaps hover:text-foreground" @click="toggle('tags')">
          <component :is="isOpen('tags') ? ChevronDownIcon : ChevronRightIcon" class="size-3" />
          <TagIcon class="size-3.5" />
          Tags
          <span class="ml-auto font-medium tracking-normal normal-case tabular-nums">{{ tags.length }}</span>
        </button>
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
              :class="cn('flex h-(--row-h) w-full cursor-pointer items-center gap-1.5 pr-2 pl-3 text-left', selectedOid === t.target ? 'row-selected' : 'hover:bg-row-hover')"
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
        <div class="mt-1 flex h-(--row-h) items-center gap-1.5 px-2 smallcaps">
          <span class="size-3" />
          <ArchiveIcon class="size-3.5" />
          Stashes
          <span class="ml-auto font-medium tracking-normal normal-case tabular-nums">{{ stashCount }}</span>
        </div>
        <!-- TODO(backend): no `stash list` command exists yet, so only `status.stashCount` is shown. -->
        <p class="h-(--row-h) px-7 leading-(--row-h) text-muted-foreground italic">
          {{ stashCount === 0 ? "No stashes" : `${stashCount} ${stashCount === 1 ? "stash" : "stashes"} (list coming later)` }}
        </p>
      </div>
    </ScrollArea>
  </section>
</template>
