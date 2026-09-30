<script setup lang="ts">
import type { PrimitiveProps } from 'reka-ui'
import type { HTMLAttributes } from 'vue'
import type { ButtonVariants } from '.'
import { Primitive } from 'reka-ui'
import { inject } from 'vue'
import { cn } from '@/lib/utils'
import { buttonSizeKey, buttonVariants } from '.'

interface Props extends PrimitiveProps {
  variant?: ButtonVariants['variant']
  size?: ButtonVariants['size']
  class?: HTMLAttributes['class']
}

const props = withDefaults(defineProps<Props>(), {
  as: 'button',
})

const inheritedSize = inject(buttonSizeKey, undefined)
</script>

<template>
  <Primitive
    data-slot="button"
    :data-variant="variant"
    :data-size="size ?? inheritedSize"
    :as="as"
    :as-child="asChild"
    :class="cn(buttonVariants({ variant, size: size ?? inheritedSize }), props.class)"
  >
    <slot />
  </Primitive>
</template>
