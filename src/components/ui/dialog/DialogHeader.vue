<script setup lang="ts">
import type { HTMLAttributes } from 'vue'
import { cn } from '@/lib/utils'

const props = defineProps<{
  class?: HTMLAttributes['class']
}>()
</script>

<template>
  <div
    data-slot="dialog-header"
    :class="cn('gap-2 flex flex-col -mx-6 border-b px-6 pb-5', props.class)"
  >
    <slot />
  </div>
</template>
