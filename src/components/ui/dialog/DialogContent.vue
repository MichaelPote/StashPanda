<script setup lang="ts">
import type { DialogContentEmits, DialogContentProps } from 'reka-ui'

import type { HTMLAttributes } from 'vue'
import { Maximize2Icon, Minimize2Icon, XIcon } from '@lucide/vue'
import { reactiveOmit } from '@vueuse/core'
import {
  DialogClose,
  DialogContent,
  DialogPortal,
  useForwardPropsEmits,
} from 'reka-ui'
import { computed, nextTick, ref, watchEffect } from 'vue'
import type { DialogSize } from '@/lib/bindings'
import { cn } from '@/lib/utils'
import { Button } from '@/components/ui/button'
import { Tip } from '@/components/ui/tooltip'
import { useUiStateStore } from '@/stores/uiState'
import { vPixelSnap } from '@/lib/pixelSnap'
import DialogOverlay from './DialogOverlay.vue'

defineOptions({
  inheritAttrs: false,
})

const props = withDefaults(defineProps<DialogContentProps & {
  class?: HTMLAttributes['class']
  showCloseButton?: boolean
  /**
   * Remembers the size the user drags the dialog to, in the UI state under `dialogSizes.<name>`.
   * Without a name the dialog still resizes but opens at its natural size each time.
   */
  sizeKey?: string
  resizable?: boolean
  /**
   * False keeps the dialog open when the user clicks the dimmed background, for dialogs where
   * a stray click would throw away work. Escape and the close button still close it.
   */
  closeOnOutsideClick?: boolean
}>(), {
  showCloseButton: true,
  resizable: true,
  closeOnOutsideClick: true,
})
const emits = defineEmits<DialogContentEmits>()

const delegatedProps = reactiveOmit(props, 'class', 'sizeKey', 'resizable', 'closeOnOutsideClick')

const forwarded = useForwardPropsEmits(delegatedProps, emits)

// ---- Resizing ------------------------------------------------------------------------------
//
// Dragging an edge or corner makes the dialog bigger. It stays centred (`inset-0 m-auto` below),
// so the size changes by twice the pointer's movement. The size is applied as a
// minimum width and height: the dialog never draws smaller than its content needs, and while
// it is taller than that the `[data-resized]` rules in index.css give the extra height to the
// parts marked `data-dialog-grow`.

const uiState = useUiStateStore()
/** The size while dragging, and the size of a dialog without a `sizeKey`. */
const localSize = ref<DialogSize | null>(null)
const dragging = ref(false)

const size = computed<DialogSize | null>(() => {
  if (dragging.value || !props.sizeKey) return localSize.value
  const saved = uiState.state?.dialogSizes?.[props.sizeKey]
  return saved && (saved.width ?? 0) > 0 && (saved.height ?? 0) > 0 ? saved : null
})

// ---- Filling the window --------------------------------------------------------------------
//
// The button beside the close button makes a resizable dialog cover the whole window, with no
// rounded corners or margin, and back. It is remembered with the dialog's size
// (`dialogSizes.<name>.maximised`), and the size dragged to is kept for when it is restored.

/** Whether a dialog without a `sizeKey` fills the window; one with a key reads the UI state. */
const localMaximised = ref(false)
const maximised = computed(() => (props.sizeKey ? uiState.state?.dialogSizes?.[props.sizeKey]?.maximised === true : localMaximised.value))

function toggleMaximised(): void {
  const next = !maximised.value
  if (props.sizeKey) uiState.patchDebounced({ dialogSizes: { [props.sizeKey]: { maximised: next } } })
  else localMaximised.value = next
}

/**
 * The resize handles, whose parent is the dialog element. The element is taken from them rather
 * than from a ref on reka-ui's DialogContent: that ref forwards an element reka-ui works out once
 * and caches, which can be the placeholder it renders while closed, so the size was sometimes
 * never applied and dragging an edge did nothing.
 */
const handleEls = ref<HTMLElement[]>([])

// Set on the element itself: reka-ui's dismissable layer writes its own `style` attribute over
// a `:style` binding here. Never wider or taller than the window, even when it shrank since.
watchEffect(() => {
  // No handles while the dialog is closed, or when it is not resizable (then it has no size).
  const el = handleEls.value[0]?.parentElement
  if (!el) return
  // Filling the window, the dialog's size is the window's.
  const s = maximised.value ? null : size.value
  el.style.minWidth = s ? `min(${s.width}px, calc(100vw - 2rem))` : ''
  el.style.minHeight = s ? `min(${s.height}px, calc(100vh - 2rem))` : ''
})

type Edge = 'n' | 's' | 'e' | 'w' | 'ne' | 'nw' | 'se' | 'sw'
const EDGES: { edge: Edge, class: string }[] = [
  { edge: 'n', class: '-top-1 inset-x-3 h-2 cursor-ns-resize' },
  { edge: 's', class: '-bottom-1 inset-x-3 h-2 cursor-ns-resize' },
  { edge: 'w', class: '-left-1 inset-y-3 w-2 cursor-ew-resize' },
  { edge: 'e', class: '-right-1 inset-y-3 w-2 cursor-ew-resize' },
  { edge: 'nw', class: '-top-1 -left-1 size-4 cursor-nwse-resize' },
  { edge: 'se', class: '-right-1 -bottom-1 size-4 cursor-nwse-resize' },
  { edge: 'ne', class: '-top-1 -right-1 size-4 cursor-nesw-resize' },
  { edge: 'sw', class: '-bottom-1 -left-1 size-4 cursor-nesw-resize' },
]

function startResize(edge: Edge, event: PointerEvent): void {
  if (event.button !== 0) return
  const handle = event.currentTarget as HTMLElement
  const dialog = handle.parentElement
  if (!dialog) return
  event.preventDefault()
  handle.setPointerCapture(event.pointerId)
  const rect = dialog.getBoundingClientRect()
  const startX = event.clientX
  const startY = event.clientY
  const dirX = edge.includes('e') ? 1 : edge.includes('w') ? -1 : 0
  const dirY = edge.includes('s') ? 1 : edge.includes('n') ? -1 : 0
  const maxW = window.innerWidth - 32
  const maxH = window.innerHeight - 32
  localSize.value = { width: Math.round(rect.width), height: Math.round(rect.height) }
  dragging.value = true

  const onMove = (e: PointerEvent): void => {
    localSize.value = {
      width: Math.round(Math.min(maxW, Math.max(0, rect.width + 2 * dirX * (e.clientX - startX)))),
      height: Math.round(Math.min(maxH, Math.max(0, rect.height + 2 * dirY * (e.clientY - startY)))),
    }
  }
  const onUp = (e: PointerEvent): void => {
    // The last movements can arrive only with the release, so size to its position too.
    onMove(e)
    window.removeEventListener('pointermove', onMove)
    window.removeEventListener('pointerup', onUp)
    window.removeEventListener('pointercancel', onUp)
    // Record what the dialog actually drew at, which is never below its natural size. Measured
    // after the next render, once the last size has reached the element's style.
    void nextTick(() => {
      const drawn = dialog.getBoundingClientRect()
      const final = { width: Math.round(drawn.width), height: Math.round(drawn.height) }
      localSize.value = final
      dragging.value = false
      if (props.sizeKey) uiState.patchDebounced({ dialogSizes: { [props.sizeKey]: final } })
    })
  }
  // On the window, so the drag carries on if the pointer outruns the thin handle.
  window.addEventListener('pointermove', onMove)
  window.addEventListener('pointerup', onUp)
  window.addEventListener('pointercancel', onUp)
}

/** Runs alongside any `interact-outside` listener the dialog itself passes. */
function onInteractOutside(event: Event): void {
  if (!props.closeOnOutsideClick) event.preventDefault()
}

/** Double-clicking an edge goes back to the natural size. */
function resetSize(): void {
  localSize.value = null
  if (props.sizeKey) uiState.patchDebounced({ dialogSizes: { [props.sizeKey]: { width: 0, height: 0 } } })
}
</script>

<!-- Kept so that its text gets ClearType (measured in headless Edge 154, 2026-10-01):
     - Absolute, not fixed, and centred by `inset-0 m-auto` rather than translate(-50%): a fixed
       element gets a compositor layer of its own, and a -50% translate puts a dialog of odd
       width or height on a half pixel.
     - No `ring` and no shadow-* class: both are a box-shadow. In the app the dialog is always on
       a layer of its own, because it is drawn over the panels' composited scrollers, and
       Chromium drops ClearType for the whole of such a layer when its element has a
       box-shadow (a translucent border does the same).
     - A flex column, not a grid: WebKit (the Linux and macOS builds) ignores `h-fit` on an
       absolutely placed grid and stretches it to the window's full height, so `m-auto` had
       nothing to centre. A flex column takes its content's height in both engines.
     - v-pixel-snap: `m-auto` still leaves the dialog on a half pixel when the room around it is
       odd; this moves it back onto whole pixels, for its 12px icons and for ClearType.
     See DialogOverlay.vue. -->
<template>
  <DialogPortal>
    <DialogOverlay />
    <DialogContent
      v-pixel-snap
      data-slot="dialog-content"
      v-bind="{ ...$attrs, ...forwarded }"
      :data-resized="size || maximised ? '' : undefined"
      :data-maximised="maximised ? '' : undefined"
      @interact-outside="onInteractOutside"
      :class="cn(
        'bg-popover text-popover-foreground data-open:animate-in data-closed:animate-out data-closed:fade-out-0 data-open:fade-in-0 data-closed:zoom-out-95 data-open:zoom-in-95 flex flex-col max-w-[calc(100%-2rem)] gap-5 rounded-xl p-6 text-sm duration-100 sm:max-w-sm absolute inset-0 m-auto z-50 h-fit w-full outline-none',
        props.class,
        // Last, so it wins over the width and height a dialog asks for.
        maximised && 'm-0 h-full w-full max-w-none sm:max-w-none rounded-none',
      )"
    >
      <slot />

      <!-- Fill the window, or go back to the dialog's own size; beside the close button. -->
      <Tip v-if="resizable" :content="maximised ? 'Restore the dialog to its own size' : 'Fill the window'">
        <Button
          variant="ghost"
          size="icon"
          :class="cn('absolute top-4', showCloseButton ? 'right-14' : 'right-4')"
          :aria-pressed="maximised"
          data-dialog-maximise
          @click="toggleMaximised"
        >
          <Minimize2Icon v-if="maximised" class="size-4" />
          <Maximize2Icon v-else class="size-4" />
          <span class="sr-only">{{ maximised ? 'Restore' : 'Maximise' }}</span>
        </Button>
      </Tip>

      <DialogClose
        v-if="showCloseButton"
        data-slot="dialog-close"
        as-child
      >
        <Button variant="ghost" class="absolute top-4 right-4" size="icon">
          <XIcon class="size-5" />
          <span class="sr-only">Close</span>
        </Button>
      </DialogClose>

      <template v-if="resizable && !maximised">
        <Tip v-for="h in EDGES" :key="h.edge" content="Drag to resize; double-click to reset">
          <div
            ref="handleEls"
            data-dialog-resize
            aria-hidden="true"
            :class="cn('absolute z-10 touch-none select-none', h.class)"
            @pointerdown="startResize(h.edge, $event)"
            @dblclick="resetSize"
          />
        </Tip>
      </template>
    </DialogContent>
  </DialogPortal>
</template>
