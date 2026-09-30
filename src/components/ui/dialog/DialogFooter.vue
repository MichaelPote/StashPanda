<script setup lang="ts">
import type { HTMLAttributes } from 'vue'
import { DialogClose } from 'reka-ui'
import { provide } from 'vue'
import { cn } from '@/lib/utils'
import { Button, buttonSizeKey } from '@/components/ui/button'

const props = withDefaults(defineProps<{
  class?: HTMLAttributes['class']
  showCloseButton?: boolean
}>(), {
  showCloseButton: false,
})

provide(buttonSizeKey, 'action')
</script>

<template>
  <div
    data-slot="dialog-footer"
    :class="cn('dialog-well -mx-6 -mb-6 rounded-b-xl border-t px-6 py-4 flex flex-col-reverse gap-2.5 sm:flex-row sm:items-center sm:justify-end', props.class)"
  >
    <slot />
    <DialogClose v-if="showCloseButton" as-child>
      <Button variant="outline">
        Close
      </Button>
    </DialogClose>
  </div>
</template>
