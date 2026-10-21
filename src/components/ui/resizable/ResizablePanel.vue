<script setup lang="ts">
import type { SplitterPanelEmits, SplitterPanelProps } from 'reka-ui'
import { SplitterPanel, useForwardExpose, useForwardPropsEmits } from 'reka-ui'
import { vPixelSnap } from '@/lib/pixelSnap'

const props = defineProps<SplitterPanelProps>()
const emits = defineEmits<SplitterPanelEmits>()

const forwarded = useForwardPropsEmits(props, emits)
useForwardExpose()
</script>

<template>
  <!-- v-pixel-snap: the splitter sizes panes in percentages; this puts each one back on whole pixels. -->
  <SplitterPanel
    v-slot="slotProps"
    v-pixel-snap
    data-slot="resizable-panel"
    v-bind="forwarded"
  >
    <slot v-bind="slotProps" />
  </SplitterPanel>
</template>
