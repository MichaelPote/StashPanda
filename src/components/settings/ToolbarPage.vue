<script setup lang="ts">
import { computed, ref, watch, type Component } from "vue";
import { useEventListener } from "@vueuse/core";
import { ChevronDownIcon, ChevronUpIcon, GripVerticalIcon, PlusIcon, RotateCcwIcon, XIcon } from "@lucide/vue";
import { Button } from "@/components/ui/button";
import { errorMessage } from "@/lib/ipc";
import {
  availableCommands,
  isDefaultToolbar,
  LAYOUT_ITEMS,
  moveToolbarItem,
  normaliseToolbar,
  toolbarCommand,
} from "@/lib/toolbarItems";
import { cn } from "@/lib/utils";
import { useSettingsStore } from "@/stores/settings";

/**
 * The Toolbar page in Settings: the toolbar's items in order, which can be dragged by their
 * handles or moved with the arrow buttons, and the items that can be added. Every change is
 * saved at once, so the toolbar behind the dialog follows it.
 */
const settingsStore = useSettingsStore();

/** The toolbar's items as drawn, by id. Ahead of the saved setting while a drag or a save is under way. */
const layout = ref<string[]>(normaliseToolbar(settingsStore.settings?.toolbar));
const error = ref<string | null>(null);
/** Where the dragged row is now; null when nothing is being dragged. */
const dragIndex = ref<number | null>(null);

watch(
  () => settingsStore.settings?.toolbar,
  (saved) => {
    if (dragIndex.value === null) layout.value = normaliseToolbar(saved);
  },
);

const available = computed(() => availableCommands(layout.value));
const isDefault = computed(() => isDefaultToolbar(layout.value));

/** The name, explanation and icon a row shows for an item id. */
function describe(id: string): { label: string; help: string | null; icon: Component } {
  const command = toolbarCommand(id);
  if (command) return { label: command.label, help: null, icon: command.icon };
  const item = LAYOUT_ITEMS.find((i) => i.id === id)!;
  return { label: item.label, help: item.help, icon: item.icon };
}

/** Shows `next` straight away and saves it. The default layout is saved as null, so a later default reaches it. */
async function save(next: string[]): Promise<void> {
  layout.value = next;
  error.value = null;
  try {
    await settingsStore.update({ toolbar: isDefaultToolbar(next) ? null : next });
  } catch (e) {
    error.value = errorMessage(e);
    layout.value = normaliseToolbar(settingsStore.settings?.toolbar);
  }
}

function add(id: string): void {
  void save([...layout.value, id]);
}

function remove(index: number): void {
  void save(layout.value.filter((_, i) => i !== index));
}

function move(from: number, to: number): void {
  void save(moveToolbarItem(layout.value, from, to));
}

function reset(): void {
  void save(normaliseToolbar(null));
}

// Dragging a row by its handle. The rows are reordered as the pointer passes over them, and the
// new order is saved when the button is released. Pointer events are used rather than HTML drag
// and drop, which the window's own file drop handling takes over on Windows.
const list = ref<HTMLElement | null>(null);
/** The order when the drag started, to tell whether it changed anything. */
let dragStart: string[] = [];

function startDrag(index: number, event: PointerEvent): void {
  if (event.button !== 0) return;
  event.preventDefault();
  dragIndex.value = index;
  dragStart = [...layout.value];
}

useEventListener(window, "pointermove", (event: PointerEvent) => {
  const from = dragIndex.value;
  if (from === null || !list.value) return;
  const rows = [...list.value.querySelectorAll<HTMLElement>("[data-toolbar-row]")];
  // The row under the pointer, or the first or last row when it is above or below the list.
  let to = rows.findIndex((row) => event.clientY < row.getBoundingClientRect().bottom);
  if (to === -1) to = rows.length - 1;
  if (to === from) return;
  layout.value = moveToolbarItem(layout.value, from, to);
  dragIndex.value = to;
});

function endDrag(): void {
  if (dragIndex.value === null) return;
  dragIndex.value = null;
  const next = layout.value;
  if (next.length !== dragStart.length || next.some((id, i) => id !== dragStart[i])) void save(next);
}
useEventListener(window, "pointerup", endDrag);
useEventListener(window, "pointercancel", endDrag);

const ROW_CLASS = "flex h-9 min-w-0 items-center gap-2 rounded-md border border-border px-2";
</script>

<template>
  <div class="grid gap-4">
    <div class="flex flex-wrap items-center gap-2">
      <p class="min-w-0 flex-1 text-base text-muted-foreground">
        Drag an item by its handle, or use its arrows, to move it along the toolbar. Right-click the toolbar to come back here.
      </p>
      <Button type="button" variant="outline" size="sm" :disabled="isDefault" @click="reset">
        <RotateCcwIcon />
        Reset to Default
      </Button>
    </div>

    <p v-if="error" class="rounded-md border border-destructive/40 bg-destructive/10 px-2.5 py-1.5 text-base break-words text-destructive" role="alert">
      {{ error }}
    </p>

    <div class="grid grid-cols-[repeat(auto-fit,minmax(18rem,1fr))] items-start gap-6">
      <section class="grid gap-2" aria-labelledby="toolbar-layout-heading">
        <h4 id="toolbar-layout-heading" class="text-base font-medium">On the toolbar, from left to right</h4>
        <p v-if="layout.length === 0" class="text-base text-muted-foreground">The toolbar is empty. Add items from the list of available items.</p>
        <ol v-else ref="list" class="grid gap-1 select-none">
          <li
            v-for="(id, index) in layout"
            :key="index"
            data-toolbar-row
            :class="cn(ROW_CLASS, dragIndex === index && 'border-primary bg-row-hover')"
          >
            <button
              type="button"
              class="shrink-0 cursor-grab touch-none rounded p-0.5 text-muted-foreground hover:bg-row-hover hover:text-foreground active:cursor-grabbing"
              :aria-label="`Drag ${describe(id).label}`"
              title="Drag to move"
              @pointerdown="startDrag(index, $event)"
            >
              <GripVerticalIcon class="size-4" />
            </button>
            <component :is="describe(id).icon" class="size-4 shrink-0 text-muted-foreground" />
            <span class="min-w-0 flex-1 truncate text-base" :title="describe(id).help ?? describe(id).label">{{ describe(id).label }}</span>
            <Button type="button" variant="ghost" size="icon-xs" :disabled="index === 0" :aria-label="`Move ${describe(id).label} up, further left`" title="Move up, further left" @click="move(index, index - 1)">
              <ChevronUpIcon />
            </Button>
            <Button
              type="button"
              variant="ghost"
              size="icon-xs"
              :disabled="index === layout.length - 1"
              :aria-label="`Move ${describe(id).label} down, further right`"
              title="Move down, further right"
              @click="move(index, index + 1)"
            >
              <ChevronDownIcon />
            </Button>
            <Button type="button" variant="ghost" size="icon-xs" :aria-label="`Remove ${describe(id).label}`" title="Remove from the toolbar" @click="remove(index)">
              <XIcon />
            </Button>
          </li>
        </ol>
      </section>

      <section class="grid gap-2" aria-labelledby="toolbar-available-heading">
        <h4 id="toolbar-available-heading" class="text-base font-medium">Available items</h4>
        <ul class="grid gap-1">
          <li v-for="item in LAYOUT_ITEMS" :key="item.id" :class="ROW_CLASS">
            <component :is="item.icon" class="size-4 shrink-0 text-muted-foreground" />
            <span class="min-w-0 flex-1 truncate text-base" :title="item.help">{{ item.label }}</span>
            <Button type="button" variant="ghost" size="xs" :aria-label="`Add a ${item.label}`" @click="add(item.id)">
              <PlusIcon />
              Add
            </Button>
          </li>
          <li v-for="command in available" :key="command.id" :class="ROW_CLASS">
            <component :is="command.icon" class="size-4 shrink-0 text-muted-foreground" />
            <span class="min-w-0 flex-1 truncate text-base" :title="command.label">{{ command.label }}</span>
            <Button type="button" variant="ghost" size="xs" :aria-label="`Add ${command.label}`" @click="add(command.id)">
              <PlusIcon />
              Add
            </Button>
          </li>
          <li v-if="available.length === 0" class="px-2 text-base text-muted-foreground">Every command is on the toolbar.</li>
        </ul>
      </section>
    </div>
  </div>
</template>
