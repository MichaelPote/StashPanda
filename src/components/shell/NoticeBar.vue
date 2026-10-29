<script setup lang="ts">
import { TriangleAlertIcon, XIcon } from "@lucide/vue";

/**
 * A bar across the window under the toolbar: an error in red, or a warning in orange, with a
 * Close key and optionally one action key, both the size of the operation banner's keys.
 * Padding in whole pixels, so the icons start on whole pixels (see index.css). Everything sits at
 * the top: the 14px icon is centred on the message's first line, and the 3px above the message
 * centres that line on the 26px keys.
 */
const props = defineProps<{
  tone: "error" | "warning";
  message: string;
  /** The label of a key that does something about it, such as "Pull…". */
  action?: string | null;
}>();

const emit = defineEmits<{ close: []; action: [] }>();

const TONES = {
  error: { bar: "border-diff-del-mark bg-diff-del text-destructive", key: "key-danger" },
  warning: { bar: "border-modified/40 bg-modified/10 text-modified-fg", key: "key-modified" },
};
</script>

<template>
  <div
    :class="['flex shrink-0 items-start gap-2 border-b px-[10px] py-[4px] text-base', TONES[props.tone].bar]"
    :role="props.tone === 'error' ? 'alert' : 'status'"
    data-notice
  >
    <TriangleAlertIcon class="mt-[5px] size-4 shrink-0" />
    <!-- Up to three lines, so a reason after Git's first line still shows; the tooltip has all of it. -->
    <span class="line-clamp-3 min-w-0 py-[3px] break-words whitespace-pre-line select-text" :title="props.message">{{ props.message }}</span>
    <span class="ml-auto flex shrink-0 items-center gap-3 pb-0.5 pl-2">
      <button v-if="props.action" type="button" :class="['bar-key', TONES[props.tone].key]" @click="emit('action')">
        {{ props.action }}
      </button>
      <button type="button" :class="['bar-key', TONES[props.tone].key]" data-notice-close @click="emit('close')">
        <XIcon class="size-3.5" />
        Close
      </button>
    </span>
  </div>
</template>
