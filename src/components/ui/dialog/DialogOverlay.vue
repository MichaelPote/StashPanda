<script setup lang="ts">
import type { DialogOverlayProps } from 'reka-ui'
import type { HTMLAttributes } from 'vue'
import { reactiveOmit } from '@vueuse/core'
import { DialogOverlay } from 'reka-ui'
import { cn } from '@/lib/utils'

const props = defineProps<DialogOverlayProps & { class?: HTMLAttributes['class'] }>()

const delegatedProps = reactiveOmit(props, 'class')
</script>

<!-- No backdrop blur, and absolute rather than fixed (the window never scrolls, so they cover the
     same area). A backdrop-filter or a fixed position gives the backdrop a compositor layer of its
     own, which makes the dialog drawn over it need one too; Chromium then keeps ClearType for the
     dialog's text only in some conditions (see DialogContent.vue). In the app the panels'
     composited scrollers behind the backdrop give it a layer anyway. Darkened to 60% black to
     make up for the blur. Measured in headless Edge 154 on 2026-10-01. -->
<template>
  <DialogOverlay
    data-slot="dialog-overlay"
    v-bind="delegatedProps"
    :class="cn('data-open:animate-in data-closed:animate-out data-closed:fade-out-0 data-open:fade-in-0 bg-black/60 duration-100 absolute inset-0 isolate z-50', props.class)"
  >
    <slot />
  </DialogOverlay>
</template>
