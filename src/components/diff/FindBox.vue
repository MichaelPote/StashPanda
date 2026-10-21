<script setup lang="ts">
import { ref } from "vue";
import { ChevronLeftIcon, ChevronRightIcon, SearchIcon, XIcon } from "@lucide/vue";
import { InputGroup, InputGroupAddon, InputGroupButton, InputGroupInput, InputGroupText } from "@/components/ui/input-group";
import { Tip } from "@/components/ui/tooltip";

/**
 * The Find box of a read-only view of lines (the diff view, the Blame dialog), which places it in
 * its top right corner; `useFind` holds what it finds. Enter and > go to the next match, and
 * Shift+Enter and < to the previous one, wrapping around at the ends; Escape closes it.
 */
defineProps<{
  /** "3 of 12", "No matches", or nothing before anything is typed. */
  summary: string;
  /** Whether there are matches to step through. */
  canStep: boolean;
  placeholder: string;
}>();

const query = defineModel<string>({ required: true });

const emit = defineEmits<{
  step: [direction: 1 | -1];
  close: [];
}>();

const root = ref<HTMLElement | null>(null);

/** Focuses the text field and selects its text, as Ctrl+F does when the box is already open. */
function focus(): void {
  const input = root.value?.querySelector("input");
  input?.focus();
  input?.select();
}

function onKeydown(event: KeyboardEvent): void {
  if (event.key === "Enter") {
    event.preventDefault();
    emit("step", event.shiftKey ? -1 : 1);
  } else if (event.key === "Escape") {
    event.preventDefault();
    emit("close");
  } else if ((event.ctrlKey || event.metaKey) && (event.key === "f" || event.key === "F")) {
    // Ctrl+F again selects the text, and never opens the web view's own find bar.
    event.preventDefault();
    focus();
  }
  // Nothing else in the view, its dialog or the window acts on keys typed here.
  event.stopPropagation();
}

defineExpose({ focus });
</script>

<template>
  <div ref="root" class="w-72 max-w-[calc(100%-2rem)] rounded-lg bg-popover shadow-md" data-diff-find @keydown="onKeydown">
    <InputGroup>
      <InputGroupAddon>
        <SearchIcon />
      </InputGroupAddon>
      <InputGroupInput v-model="query" :placeholder="placeholder" :aria-label="placeholder" spellcheck="false" />
      <InputGroupAddon align="inline-end">
        <InputGroupText v-if="summary" class="text-xs tabular-nums" data-diff-find-count>{{ summary }}</InputGroupText>
        <!-- A disabled button lets no pointer events through, so the spans carry the tooltips. -->
        <Tip content="Previous match (Shift+Enter)">
          <span class="inline-flex shrink-0">
            <InputGroupButton size="icon-xs" aria-label="Previous match" :disabled="!canStep" @click="emit('step', -1)">
              <ChevronLeftIcon />
            </InputGroupButton>
          </span>
        </Tip>
        <Tip content="Next match (Enter)">
          <span class="inline-flex shrink-0">
            <InputGroupButton size="icon-xs" aria-label="Next match" :disabled="!canStep" @click="emit('step', 1)">
              <ChevronRightIcon />
            </InputGroupButton>
          </span>
        </Tip>
        <Tip content="Close (Escape)">
          <InputGroupButton size="icon-xs" aria-label="Close" @click="emit('close')">
            <XIcon />
          </InputGroupButton>
        </Tip>
      </InputGroupAddon>
    </InputGroup>
  </div>
</template>
