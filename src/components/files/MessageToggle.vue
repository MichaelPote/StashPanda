<script setup lang="ts">
import { ChevronDownIcon, ChevronRightIcon } from "@lucide/vue";

defineProps<{
  expanded: boolean;
}>();

const emit = defineEmits<{
  toggle: [];
}>();
</script>

<template>
  <!-- More or Less for the commit message body, in the bottom right corner of the commit header. -->
  <button
    type="button"
    class="inline-flex h-[18px] shrink-0 items-center gap-0.5 text-xs text-muted-foreground hover:text-foreground"
    :aria-expanded="expanded"
    @click="emit('toggle')"
  >
    {{ expanded ? "Less" : "More" }}
    <component :is="expanded ? ChevronDownIcon : ChevronRightIcon" class="size-3 shrink-0" />
  </button>
</template>
