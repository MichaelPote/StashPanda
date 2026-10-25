<script setup lang="ts">
import { computed, onMounted, ref } from "vue";
import { Input } from "@/components/ui/input";
import { parseColor, themeColor, toHex6, toHex8, type Rgba } from "@/lib/diff/colors";
import { BUNDLED_DIFF_FONTS, cssFontFamily } from "@/lib/diff/font";
import { installedMonospaceFonts } from "@/lib/fonts";
import { errorMessage } from "@/lib/ipc";
import { cn } from "@/lib/utils";
import { fieldId, type FormValue, type SettingsFieldDef } from "./sections";

const props = defineProps<{
  field: SettingsFieldDef;
  /** Why the current value cannot be saved, or null. */
  error: string | null;
  /** The value differs from the saved one. */
  changed: boolean;
}>();

const model = defineModel<FormValue>({ required: true });

const id = computed(() => `settings-${fieldId(props.field).replace(".", "-")}`);
const helpId = computed(() => `${id.value}-help`);
const text = computed({
  get: () => String(model.value),
  set: (v: string) => {
    model.value = v;
  },
});

// ---- Font lists ----------------------------------------------------------------------------

/** Installed monospaced fonts, or null while the scan runs. */
const installed = ref<string[] | null>(null);
const fontError = ref<string | null>(null);

onMounted(async () => {
  if (props.field.kind !== "font") return;
  try {
    const bundled = new Set<string>(BUNDLED_DIFF_FONTS);
    installed.value = (await installedMonospaceFonts()).filter((name) => !bundled.has(name));
  } catch (e) {
    installed.value = [];
    fontError.value = `Could not list the installed fonts: ${errorMessage(e)}`;
  }
});

/**
 * The saved font when it is in neither list: shown as it is while the scan runs, then marked
 * "(not installed)" if the scan does not find it (a font from another computer's settings.json).
 */
const extraFont = computed<{ name: string; label: string } | null>(() => {
  const name = text.value;
  if (name.length === 0 || (BUNDLED_DIFF_FONTS as readonly string[]).includes(name)) return null;
  if (installed.value === null) return { name, label: name };
  return installed.value.includes(name) ? null : { name, label: `${name} (not installed)` };
});

// ---- Colours ---------------------------------------------------------------------------------
//
// The swatch, the opacity slider and the text box all show the one value in the box; an empty box
// shows the theme's own colour, which the page's colour fields belong to.

/** The theme's own colour, read once: the page draws only the current theme's colour fields. */
const themeDefault = computed<Rgba | null>(() => (props.field.kind === "color" ? themeColor(props.field.cssVar) : null));
/** The colour the box holds, or the theme's while it is empty or not yet a colour. */
const shownColor = computed<Rgba>(() => parseColor(text.value) ?? themeDefault.value ?? { r: 0, g: 0, b: 0, a: 1 });
const opacityPercent = computed(() => Math.round(shownColor.value.a * 100));

function onPickColor(event: Event): void {
  const picked = parseColor((event.target as HTMLInputElement).value);
  if (picked) model.value = toHex8({ ...picked, a: shownColor.value.a });
}

function onPickOpacity(event: Event): void {
  model.value = toHex8({ ...shownColor.value, a: Number((event.target as HTMLInputElement).value) / 100 });
}

const APPLIES_TEXT = {
  unused: "Not used yet",
  reopen: "Applies when a repository is opened",
} as const;
</script>

<template>
  <div class="grid gap-1.5">
    <!-- A toggle puts its checkbox before the label; every other field puts the label above the control. -->
    <label v-if="field.kind === 'toggle'" :for="id" class="flex items-center gap-2 text-base select-none">
      <input :id="id" v-model="model" type="checkbox" class="size-3.5 accent-primary" :aria-describedby="field.help ? helpId : undefined" />
      <span :class="cn(changed && 'font-medium')">{{ field.label }}</span>
      <span v-if="field.applies" class="text-xs text-muted-foreground">({{ APPLIES_TEXT[field.applies] }})</span>
    </label>

    <!-- A colour is one row: its label, then the swatch, the opacity and the text box. -->
    <div v-else-if="field.kind === 'color'" class="flex flex-wrap items-center gap-2">
      <label :for="id" :class="cn('w-56 text-base select-none', changed && 'font-medium')">{{ field.label }}</label>
      <input
        type="color"
        class="h-7 w-10 shrink-0 cursor-pointer rounded-md border border-border bg-transparent p-0.5"
        :value="toHex6(shownColor)"
        :aria-label="`${field.label}: colour`"
        @input="onPickColor"
      />
      <input
        type="range"
        min="0"
        max="100"
        class="w-28 accent-primary"
        :value="opacityPercent"
        :aria-label="`${field.label}: opacity`"
        @input="onPickOpacity"
      />
      <span class="w-10 text-right text-base text-muted-foreground tabular-nums">{{ opacityPercent }}%</span>
      <Input
        :id="id"
        v-model="text"
        class="w-28 font-mono text-base"
        :placeholder="themeDefault ? toHex8(themeDefault) : undefined"
        autocomplete="off"
        spellcheck="false"
        :aria-invalid="error !== null || undefined"
        :aria-describedby="field.help ? helpId : undefined"
      />
      <button v-if="text.length > 0" type="button" class="text-base text-muted-foreground underline-offset-2 hover:text-foreground hover:underline" @click="model = ''">
        Reset
      </button>
    </div>

    <template v-else>
      <label :for="id" class="flex items-baseline gap-2 text-base select-none">
        <span :class="cn(changed && 'font-medium')">{{ field.label }}</span>
        <span v-if="field.applies" class="text-xs text-muted-foreground">({{ APPLIES_TEXT[field.applies] }})</span>
      </label>

      <select
        v-if="field.kind === 'select'"
        :id="id"
        v-model="text"
        class="well h-8 w-full max-w-sm min-w-0 rounded-lg border border-border px-2 text-base outline-none focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/50"
        :aria-describedby="field.help ? helpId : undefined"
      >
        <option v-for="option in field.options" :key="option.value" :value="option.value">{{ option.label }}</option>
      </select>

      <template v-else-if="field.kind === 'font'">
        <select
          :id="id"
          v-model="text"
          class="well h-8 w-full max-w-sm min-w-0 rounded-lg border border-border px-2 text-base outline-none focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/50"
          :style="{ fontFamily: text ? cssFontFamily(text) : undefined }"
          :aria-describedby="field.help ? helpId : undefined"
        >
          <option v-if="extraFont" :value="extraFont.name">{{ extraFont.label }}</option>
          <optgroup label="Bundled with StashPanda">
            <option v-for="name in BUNDLED_DIFF_FONTS" :key="name" :value="name" :style="{ fontFamily: cssFontFamily(name) }">{{ name }}</option>
          </optgroup>
          <optgroup :label="installed === null ? 'Looking for installed fonts…' : 'Installed on this computer'">
            <option v-if="installed !== null && installed.length === 0" disabled value="">No other monospaced fonts found</option>
            <option v-for="name in installed ?? []" :key="name" :value="name" :style="{ fontFamily: cssFontFamily(name) }">{{ name }}</option>
          </optgroup>
        </select>
        <p v-if="fontError" class="text-xs text-destructive">{{ fontError }}</p>
      </template>

      <div v-else-if="field.kind === 'number'" class="flex items-center gap-2">
        <Input
          :id="id"
          v-model="text"
          type="number"
          inputmode="decimal"
          class="w-32 text-base tabular-nums"
          :min="field.min"
          :max="field.max"
          :step="field.step ?? 1"
          :placeholder="field.emptyLabel"
          :aria-invalid="error !== null || undefined"
          :aria-describedby="field.help ? helpId : undefined"
        />
        <span v-if="field.unit" class="text-base text-muted-foreground">{{ field.unit }}</span>
      </div>

      <Input
        v-else
        :id="id"
        v-model="text"
        :class="cn('text-base', field.monospace && 'font-mono')"
        :placeholder="field.placeholder"
        autocomplete="off"
        spellcheck="false"
        :aria-invalid="error !== null || undefined"
        :aria-describedby="field.help ? helpId : undefined"
      />
    </template>

    <p v-if="error" class="text-xs text-destructive">{{ error }}</p>
    <p v-if="field.help" :id="helpId" :class="cn('text-xs text-muted-foreground', field.kind === 'toggle' && 'pl-5.5')">{{ field.help }}</p>
  </div>
</template>
