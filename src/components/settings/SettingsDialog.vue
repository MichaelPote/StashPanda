<script setup lang="ts">
import { computed, reactive, ref, toRaw, watch } from "vue";
import { CircleAlertIcon, LoaderCircleIcon } from "@lucide/vue";
import type { Settings } from "@/lib/bindings";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { errorMessage } from "@/lib/ipc";
import type { JsonObject } from "@/lib/mergePatch";
import { cn } from "@/lib/utils";
import { useDialogsStore } from "@/stores/dialogs";
import { useSessionsStore } from "@/stores/sessions";
import { useSettingsStore } from "@/stores/settings";
import SettingsField from "./SettingsField.vue";
import {
  fieldId,
  fromFormValue,
  readStored,
  SETTINGS_SECTIONS,
  toFormValue,
  type FormValue,
  type SettingsFieldDef,
  type SettingsSection,
} from "./sections";

/** Saving any of these rebuilds the graph of every open tab, since the backend reads them only then. */
const GRAPH_FIELDS = new Set(["appearance.graphOrder", "appearance.commitLaneColours", "cache.maxCommitsPerRepo"]);

const dialogs = useDialogsStore();
const sessions = useSessionsStore();
const settingsStore = useSettingsStore();

const request = computed(() => dialogs.current("settings"));

const sectionId = ref(SETTINGS_SECTIONS[0]!.id);
/** The settings as they were when the dialog opened; changes are measured against these. */
const original = ref<Settings | null>(null);
/** What each field holds, keyed by `fieldId`. */
const values = reactive<Record<string, FormValue>>({});
const error = ref<string | null>(null);
const saving = ref(false);

watch(request, (r) => {
  if (!r) return;
  const current = settingsStore.settings;
  // The store holds a reactive proxy, which structuredClone refuses; clone the plain object under it.
  original.value = current ? structuredClone(toRaw(current)) : null;
  for (const section of SETTINGS_SECTIONS) {
    for (const field of section.fields) {
      values[fieldId(field)] = toFormValue(field, current ? readStored(current, field) : undefined);
    }
  }
  sectionId.value = SETTINGS_SECTIONS.some((s) => s.id === r.section) ? r.section! : SETTINGS_SECTIONS[0]!.id;
  error.value = null;
  saving.value = false;
});

const section = computed<SettingsSection>(() => SETTINGS_SECTIONS.find((s) => s.id === sectionId.value) ?? SETTINGS_SECTIONS[0]!);

/** The page's fields in the order drawn: one at a time, or toggles next to each other with the same `list` together. */
const blocks = computed(() => {
  const out: { key: string; list: string | null; help: string | null; fields: SettingsFieldDef[] }[] = [];
  for (const field of section.value.fields) {
    const list = field.kind === "toggle" ? (field.list ?? null) : null;
    const last = out[out.length - 1];
    if (list !== null && last?.list === list) {
      last.fields.push(field);
    } else {
      out.push({ key: fieldId(field), list, help: field.kind === "toggle" ? (field.listHelp ?? null) : null, fields: [field] });
    }
  }
  return out;
});

/** Each field's stored value from the form, or why it cannot be saved. */
const parsed = computed(() => {
  const result = new Map<string, { value: unknown } | { error: string }>();
  for (const s of SETTINGS_SECTIONS) {
    for (const field of s.fields) result.set(fieldId(field), fromFormValue(field, values[fieldId(field)] ?? ""));
  }
  return result;
});

function fieldError(field: SettingsFieldDef): string | null {
  const p = parsed.value.get(fieldId(field));
  return p && "error" in p ? p.error : null;
}

function fieldChanged(field: SettingsFieldDef): boolean {
  const p = parsed.value.get(fieldId(field));
  if (!p || "error" in p || !original.value) return false;
  // Compare with the saved value as the form would give it back, so a missing optional value
  // (undefined against null) and float noise in an f32 setting do not count as changes.
  const saved = fromFormValue(field, toFormValue(field, readStored(original.value, field)));
  return "error" in saved || p.value !== saved.value;
}

const changedFields = computed(() => SETTINGS_SECTIONS.flatMap((s) => s.fields.filter(fieldChanged)));
const hasErrors = computed(() => SETTINGS_SECTIONS.some((s) => s.fields.some((f) => fieldError(f) !== null)));
const canSave = computed(() => original.value !== null && changedFields.value.length > 0 && !hasErrors.value && !saving.value);

function sectionHasError(s: SettingsSection): boolean {
  return s.fields.some((f) => fieldError(f) !== null);
}

function sectionChanged(s: SettingsSection): boolean {
  return s.fields.some(fieldChanged);
}

const nav = ref<HTMLElement | null>(null);

/** Focuses the open page's entry in the list, rather than the first button in the dialog. */
function focusCurrentPage(event: Event): void {
  const button = nav.value?.querySelector<HTMLElement>('[aria-current="page"]');
  if (!button) return;
  event.preventDefault();
  button.focus();
}

function onOpenChange(open: boolean): void {
  if (!open) dialogs.cancel();
}

async function save(): Promise<void> {
  if (!canSave.value) return;
  // Only the changed fields go in the merge patch, so a setting changed elsewhere while the
  // dialog was open (the theme from the View menu, a library edit) is kept. A null value
  // deletes the key, which the backend reads back as "not set".
  const patch: Record<string, JsonObject> = {};
  for (const field of changedFields.value) {
    const p = parsed.value.get(fieldId(field));
    if (!p || "error" in p) continue;
    (patch[field.group] ??= {})[field.key] = p.value;
  }
  const rebuildGraphs = changedFields.value.some((f) => GRAPH_FIELDS.has(fieldId(f)));
  saving.value = true;
  error.value = null;
  try {
    await settingsStore.update(patch);
    dialogs.resolve("settings", true);
  } catch (e) {
    error.value = errorMessage(e);
    return;
  } finally {
    saving.value = false;
  }
  if (rebuildGraphs) {
    for (const session of sessions.list) void sessions.refreshSession(session.key);
  }
}
</script>

<template>
  <Dialog :open="request !== null" @update:open="onOpenChange">
    <!-- The dialog opens at 80% of the window each way; its body takes the height between the header and footer. -->
    <DialogContent v-if="request" class="h-[80vh] sm:max-w-[80vw]" size-key="settings" :close-on-outside-click="false" @open-auto-focus="focusCurrentPage">
      <DialogHeader>
        <DialogTitle>Settings</DialogTitle>
        <DialogDescription>Saved to settings.json, which you can also edit by hand.</DialogDescription>
      </DialogHeader>

      <p v-if="original === null" class="text-base text-muted-foreground">The settings have not loaded yet.</p>

      <!-- The negative margins run the page list's border from the header border to the footer. -->
      <div v-else class="-mx-6 -my-5 grid min-h-0 flex-1 grid-cols-[12rem_minmax(0,1fr)]" data-dialog-grow>
        <nav ref="nav" aria-label="Settings pages" class="flex flex-col gap-0.5 overflow-y-auto border-r border-border px-3 py-4 scrollbar-thin">
          <button
            v-for="s in SETTINGS_SECTIONS"
            :key="s.id"
            type="button"
            :aria-current="s.id === sectionId ? 'page' : undefined"
            :class="
              cn(
                'flex h-8 w-full shrink-0 items-center gap-2 rounded-md px-2 text-left text-base outline-none select-none focus-visible:ring-2 focus-visible:ring-ring/50',
                s.id === sectionId ? 'row-selected font-medium' : 'hover:bg-row-hover',
              )
            "
            @click="sectionId = s.id"
          >
            <component :is="s.icon" class="size-4 shrink-0 text-muted-foreground" />
            <span class="min-w-0 flex-1 truncate">{{ s.label }}</span>
            <CircleAlertIcon v-if="sectionHasError(s)" class="size-3.5 shrink-0 text-destructive" aria-label="Has a value that cannot be saved" />
            <span v-else-if="sectionChanged(s)" class="size-1.5 shrink-0 rounded-full bg-primary" aria-label="Has unsaved changes" />
          </button>
        </nav>

        <!-- A page of its own (SSH Keys) scrolls under its title; it has forms of its own, so it is not inside the settings form. -->
        <div v-if="section.page" class="flex min-h-0 flex-col">
          <div class="shrink-0 px-6 pt-4 pb-4">
            <h3 class="text-lg font-semibold">{{ section.label }}</h3>
            <p class="text-base text-muted-foreground">{{ section.description }}</p>
          </div>
          <div class="min-h-0 flex-1 overflow-y-auto px-6 pb-4 scrollbar-thin">
            <component :is="section.page" />
          </div>
        </div>

        <!-- The page's title, description and preview stay put; the fields scroll in the panel below them. -->
        <form v-else class="flex min-h-0 flex-col" @submit.prevent="save">
          <div class="shrink-0 px-6 pt-4 pb-4">
            <h3 class="text-lg font-semibold">{{ section.label }}</h3>
            <p class="text-base text-muted-foreground">{{ section.description }}</p>
            <component :is="section.preview" v-if="section.preview" :values="values" class="mt-4" />
          </div>
          <div :class="cn('min-h-0 flex-1 overflow-y-auto px-6 pb-4 scrollbar-thin', section.preview && 'border-t border-border pt-4')">
            <div class="grid gap-4">
              <template v-for="block in blocks" :key="block.key">
                <fieldset v-if="block.list" class="grid gap-1.5">
                  <legend class="mb-1.5 text-base">{{ block.list }}</legend>
                  <SettingsField
                    v-for="field in block.fields"
                    :key="fieldId(field)"
                    v-model="values[fieldId(field)]!"
                    :field="field"
                    :error="fieldError(field)"
                    :changed="fieldChanged(field)"
                  />
                  <p v-if="block.help" class="text-xs text-muted-foreground">{{ block.help }}</p>
                </fieldset>
                <SettingsField
                  v-else
                  v-model="values[fieldId(block.fields[0]!)]!"
                  :field="block.fields[0]!"
                  :error="fieldError(block.fields[0]!)"
                  :changed="fieldChanged(block.fields[0]!)"
                />
              </template>
            </div>
          </div>
          <!-- Lets Enter in a text box save, as in the other dialogs. -->
          <button type="submit" class="hidden" tabindex="-1" aria-hidden="true" />
        </form>
      </div>

      <DialogFooter>
        <p v-if="error" class="mr-auto min-w-0 text-base break-words text-destructive">{{ error }}</p>
        <Button type="button" variant="outline" @click="dialogs.cancel()">Cancel</Button>
        <Button type="button" :disabled="!canSave" @click="save">
          <LoaderCircleIcon v-if="saving" class="animate-spin" />
          Save Changes
        </Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
</template>
