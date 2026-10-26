<script setup lang="ts">
import { computed, reactive, ref, watch } from "vue";
import { LoaderCircleIcon, RotateCcwIcon, Trash2Icon } from "@lucide/vue";
import { commands, type ConfigChange, type ConfigEntry, type ConfigScope } from "@/lib/bindings";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Tabs, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { Tip } from "@/components/ui/tooltip";
import { errorMessage, unwrap } from "@/lib/ipc";
import { GIT_SETTINGS, SETTING_GROUPS, describeValue, otherEntries, parseBool, settingValues, type GitSetting } from "@/lib/gitSettings";
import { opFailureText, runOp } from "@/lib/repoActions";
import { useDialogsStore } from "@/stores/dialogs";
import { useSessionsStore } from "@/stores/sessions";

/**
 * Repository Settings: the Git configuration of one repository, in two tabs: its own file
 * (.git/config) and the global file of the machine whose Git runs it. The common settings come
 * with explanations (lib/gitSettings.ts); everything else in the file is under Other settings.
 * Changes wait for Save Changes, then go to Git one file at a time.
 */
const dialogs = useDialogsStore();
const sessions = useSessionsStore();

const request = computed(() => dialogs.current("repoSettings"));
const session = computed(() => (request.value ? (sessions.sessions.get(request.value.key) ?? null) : null));

const scope = ref<ConfigScope>("local");
const entries = ref<ConfigEntry[] | null>(null);
const loadError = ref<string | null>(null);
const saveError = ref<string | null>(null);
const saving = ref(false);

/**
 * Pending changes, by file and then by a key of their own: a catalogue setting's key in lower case,
 * or `other:<index>` for a row of Other settings, or `add:<n>` for an added one.
 */
const pending = reactive<Record<ConfigScope, Map<string, ConfigChange>>>({ local: new Map(), global: new Map() });
/** Values typed into text fields and Other settings rows, by the same keys, before they become changes. */
const drafts = reactive<Record<ConfigScope, Map<string, string>>>({ local: new Map(), global: new Map() });
const newKey = ref("");
const newValue = ref("");
let added = 0;

async function load(): Promise<void> {
  const r = request.value;
  if (!r) return;
  loadError.value = null;
  try {
    entries.value = await unwrap(commands.getGitConfig(r.key));
  } catch (e) {
    loadError.value = errorMessage(e);
  }
}

watch(request, (r) => {
  if (!r) return;
  scope.value = r.scope ?? "local";
  entries.value = null;
  saveError.value = null;
  saving.value = false;
  for (const s of ["local", "global"] as const) {
    pending[s].clear();
    drafts[s].clear();
  }
  newKey.value = "";
  newValue.value = "";
  void load();
});

/** The machine whose global file the Global tab edits, in words. */
const machine = computed(() => {
  const t = session.value?.info.transport;
  if (!t) return "this computer";
  if (t.type === "wsl") return `WSL: ${t.distro}`;
  if (t.type === "ssh") return t.host;
  return /\.exe$/i.test(t.gitExecutable) && !/^[A-Za-z]:/.test(t.gitExecutable) ? "Windows" : "this computer";
});

const groups = computed(() =>
  SETTING_GROUPS.map((group) => ({ group, settings: GIT_SETTINGS.filter((s) => s.group === group && (scope.value === "global" || !s.globalOnly)) })).filter(
    (g) => g.settings.length > 0,
  ),
);

function values(setting: GitSetting) {
  return settingValues(entries.value ?? [], setting.key, scope.value);
}

/**
 * The line under a setting's control: what applies when this file does not set it ("Uses On, from
 * the global settings", "Uses Git's default: off"), or what this file's value overrides.
 */
function sourceNote(setting: GitSetting): string {
  const { inherited } = values(setting);
  const below = inherited.value === null ? `Git's default: ${setting.gitDefault}` : `${describeValue(setting, inherited.value)}, from the ${inherited.scope} settings`;
  if (!isSetHere(setting)) return inherited.value === null ? `Uses ${below}` : `Uses ${below}`;
  return `Set here, in place of ${below}`;
}

/** The value a setting's control shows: a pending change, else what this file sets ("" when it sets nothing). */
function shown(setting: GitSetting): string {
  const id = setting.key.toLowerCase();
  const change = pending[scope.value].get(id);
  if (change) return change.type === "set" ? change.value : "";
  const own = values(setting).own.value;
  if (own === null) return "";
  if (setting.kind.type === "bool") {
    const b = parseBool(own);
    return b === null ? own : String(b);
  }
  return setting.kind.type === "choice" ? own.toLowerCase() : own;
}

/** Records the control's new value as a change, or drops the change when it matches the file again. */
function choose(setting: GitSetting, value: string): void {
  const id = setting.key.toLowerCase();
  const own = values(setting).own.value;
  const normalisedOwn = own === null ? "" : setting.kind.type === "bool" ? String(parseBool(own) ?? own) : own;
  if (value === normalisedOwn) {
    pending[scope.value].delete(id);
    return;
  }
  pending[scope.value].set(id, value === "" ? { type: "unset", key: setting.key, value: null } : { type: "set", key: setting.key, value });
}

/** A choice setting's options, plus the file's own value when it is not one of them, so it is not lost. */
function choiceOptions(setting: GitSetting): { value: string; label: string }[] {
  if (setting.kind.type !== "choice") return [];
  const own = shown(setting);
  const known = setting.kind.choices.some((c) => c.value === own);
  return known || own === "" ? setting.kind.choices : [...setting.kind.choices, { value: own, label: own }];
}

const isSetHere = (setting: GitSetting) => shown(setting) !== "";

/** Other settings in this file, with their row ids. */
const others = computed(() => otherEntries(entries.value ?? [], scope.value).map((e, i) => ({ id: `other:${i}`, entry: e })));

function otherDraft(id: string, entry: ConfigEntry): string {
  return drafts[scope.value].get(id) ?? entry.value ?? "";
}
function editOther(id: string, entry: ConfigEntry, value: string): void {
  drafts[scope.value].set(id, value);
  if (entry.value !== null && value !== entry.value) pending[scope.value].set(id, { type: "replace", key: entry.key, old: entry.value, value });
  else pending[scope.value].delete(id);
}
function removeOther(id: string, entry: ConfigEntry): void {
  pending[scope.value].set(id, { type: "unset", key: entry.key, value: entry.value });
}
const removing = (id: string) => pending[scope.value].get(id)?.type === "unset";
function restoreOther(id: string): void {
  pending[scope.value].delete(id);
  drafts[scope.value].delete(id);
}

/** Added settings waiting for Save, in this tab. */
const additions = computed(() =>
  [...pending[scope.value].entries()].filter(([id]) => id.startsWith("add:")).map(([id, change]) => ({ id, change: change as Extract<ConfigChange, { type: "add" }> })),
);

/** Whether `newKey` is a name Git would take: section.name, letters, digits and "-", the name starting with a letter. */
const newKeyProblem = computed(() => {
  const k = newKey.value.trim();
  if (k === "") return null;
  const m = /^([A-Za-z0-9][A-Za-z0-9-]*)(\.(.+))?\.([A-Za-z][A-Za-z0-9-]*)$/.exec(k);
  return m && !/[\r\n]/.test(k) ? null : "A setting's name is a section and a name, such as alias.st or remote.origin.url.";
});
function addSetting(): void {
  const k = newKey.value.trim();
  if (k === "" || newKeyProblem.value) return;
  pending[scope.value].set(`add:${added++}`, { type: "add", key: k, value: newValue.value });
  newKey.value = "";
  newValue.value = "";
}

const changeCount = computed(() => pending.local.size + pending.global.size);

function onOpenChange(open: boolean): void {
  if (!open) dialogs.cancel();
}

async function save(): Promise<void> {
  const r = request.value;
  if (!r || changeCount.value === 0 || saving.value) return;
  saving.value = true;
  saveError.value = null;
  try {
    for (const s of ["local", "global"] as const) {
      const changes = [...pending[s].values()];
      if (changes.length === 0) continue;
      const result = await runOp(r.key, () => commands.opConfigChange(r.key, s, changes));
      if (!result.ok) {
        saveError.value = opFailureText(result, `Saving the ${s === "local" ? "repository's" : "global"} settings`);
        scope.value = s;
        await load();
        return;
      }
      pending[s].clear();
      drafts[s].clear();
    }
    sessions.notifySuccess("Saved the Git settings.");
    dialogs.resolve("repoSettings", null);
  } catch (e) {
    saveError.value = errorMessage(e);
  } finally {
    saving.value = false;
  }
}

const selectClass =
  "well h-8 w-full rounded-lg border px-2.5 text-base outline-none focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/50";
</script>

<template>
  <Dialog :open="request !== null" @update:open="onOpenChange">
    <DialogContent v-if="request" class="h-[80vh] sm:max-w-3xl" size-key="repoSettings" :close-on-outside-click="false">
      <DialogHeader>
        <DialogTitle>Repository settings</DialogTitle>
        <DialogDescription>
          Git's own settings for <span class="font-mono">{{ session?.info.displayName ?? "this repository" }}</span>. A setting in the repository's
          file wins over the same setting in the global file.
        </DialogDescription>
      </DialogHeader>

      <Tabs v-model="scope">
        <TabsList class="w-full">
          <TabsTrigger value="local" class="text-base">This repository<span v-if="pending.local.size" class="ml-1">•</span></TabsTrigger>
          <TabsTrigger value="global" class="text-base">Global ({{ machine }})<span v-if="pending.global.size" class="ml-1">•</span></TabsTrigger>
        </TabsList>
      </Tabs>

      <div class="-mx-1 grid min-h-0 flex-1 content-start gap-5 overflow-y-auto px-1 scrollbar-thin" data-dialog-grow>
        <p v-if="loadError" class="text-base text-destructive">Could not read the settings: {{ loadError }}</p>
        <p v-else-if="entries === null" class="flex items-center gap-2 text-muted-foreground"><LoaderCircleIcon class="size-4 animate-spin" /> Reading the settings…</p>
        <template v-else>
          <p v-if="scope === 'global'" class="text-xs text-muted-foreground">
            These apply to every repository whose Git runs on {{ machine === "this computer" ? "this computer" : machine }}, unless a repository sets them itself.
          </p>

          <section v-for="g in groups" :key="g.group" class="grid gap-2">
            <h3 class="smallcaps text-muted-foreground">{{ g.group }}</h3>
            <div v-for="s in g.settings" :key="s.key" class="grid grid-cols-[minmax(0,1fr)_minmax(0,17rem)_1.75rem] items-start gap-x-3 gap-y-1">
              <div class="min-w-0">
                <p class="font-mono text-base">{{ s.key }}</p>
                <p class="text-xs text-muted-foreground">{{ s.description }}</p>
              </div>
              <div class="grid min-w-0 gap-1">
                <select
                  v-if="s.kind.type === 'bool'"
                  :value="shown(s)"
                  :class="selectClass"
                  :aria-label="s.key"
                  @change="choose(s, ($event.target as HTMLSelectElement).value)"
                >
                  <option value="">Not set</option>
                  <option value="true">On</option>
                  <option value="false">Off</option>
                </select>
                <select
                  v-else-if="s.kind.type === 'choice'"
                  :value="shown(s)"
                  :class="selectClass"
                  :aria-label="s.key"
                  @change="choose(s, ($event.target as HTMLSelectElement).value)"
                >
                  <option value="">Not set</option>
                  <option v-for="c in choiceOptions(s)" :key="c.value" :value="c.value">{{ c.label }}</option>
                </select>
                <Input
                  v-else
                  :model-value="shown(s)"
                  class="text-base"
                  placeholder="Not set"
                  :aria-label="s.key"
                  autocomplete="off"
                  spellcheck="false"
                  @update:model-value="choose(s, String($event).trim())"
                />
                <p class="text-xs text-muted-foreground">{{ sourceNote(s) }}</p>
              </div>
              <Tip :content="isSetHere(s) ? 'Remove it from this file, so the setting is inherited again' : null" side="left">
                <Button v-if="isSetHere(s)" type="button" variant="ghost" size="icon" class="size-7" :aria-label="`Reset ${s.key}`" @click="choose(s, '')">
                  <RotateCcwIcon class="size-3.5" />
                </Button>
              </Tip>
            </div>
          </section>

          <section class="grid gap-2">
            <h3 class="smallcaps text-muted-foreground">Other settings</h3>
            <p v-if="others.length === 0 && additions.length === 0" class="text-xs text-muted-foreground italic">No other settings in this file.</p>
            <div v-for="o in others" :key="o.id" class="grid grid-cols-[minmax(0,1fr)_minmax(0,17rem)_1.75rem] items-center gap-x-3">
              <p :class="['truncate font-mono text-base', removing(o.id) && 'text-muted-foreground line-through']" :title="o.entry.key">{{ o.entry.key }}</p>
              <Input
                :model-value="otherDraft(o.id, o.entry)"
                class="font-mono text-base"
                :disabled="removing(o.id) || o.entry.value === null"
                :placeholder="o.entry.value === null ? '(no value: true)' : undefined"
                :aria-label="o.entry.key"
                autocomplete="off"
                spellcheck="false"
                @update:model-value="editOther(o.id, o.entry, String($event))"
              />
              <Button v-if="removing(o.id)" type="button" variant="ghost" size="icon" class="size-7" :aria-label="`Keep ${o.entry.key}`" @click="restoreOther(o.id)">
                <RotateCcwIcon class="size-3.5" />
              </Button>
              <Button v-else type="button" variant="ghost" size="icon" class="size-7" :aria-label="`Remove ${o.entry.key}`" @click="removeOther(o.id, o.entry)">
                <Trash2Icon class="size-3.5" />
              </Button>
            </div>
            <div v-for="a in additions" :key="a.id" class="grid grid-cols-[minmax(0,1fr)_minmax(0,17rem)_1.75rem] items-center gap-x-3">
              <p class="truncate font-mono text-base text-added-fg">{{ a.change.key }}</p>
              <p class="truncate font-mono text-base">{{ a.change.value }}</p>
              <Button type="button" variant="ghost" size="icon" class="size-7" :aria-label="`Do not add ${a.change.key}`" @click="pending[scope].delete(a.id)">
                <Trash2Icon class="size-3.5" />
              </Button>
            </div>
            <form class="grid grid-cols-[minmax(0,1fr)_minmax(0,17rem)_auto] items-start gap-x-3 gap-y-1" @submit.prevent="addSetting">
              <Input v-model="newKey" class="font-mono text-base" placeholder="alias.st" aria-label="New setting's name" autocomplete="off" spellcheck="false" />
              <Input v-model="newValue" class="font-mono text-base" placeholder="status --short" aria-label="New setting's value" autocomplete="off" spellcheck="false" />
              <Button type="submit" variant="outline" :disabled="newKey.trim() === '' || newKeyProblem !== null">Add Setting</Button>
              <p v-if="newKeyProblem" class="col-span-3 text-xs text-destructive">{{ newKeyProblem }}</p>
            </form>
          </section>
        </template>
      </div>

      <p v-if="saveError" class="rounded-md border border-destructive/40 bg-destructive/10 px-2.5 py-1.5 text-base break-words text-destructive">{{ saveError }}</p>

      <DialogFooter>
        <Button type="button" variant="outline" @click="dialogs.cancel()">Cancel</Button>
        <Button type="button" :disabled="changeCount === 0 || saving" data-repo-settings-save @click="save">
          <LoaderCircleIcon v-if="saving" class="animate-spin" />
          Save Changes
        </Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
</template>
