<script setup lang="ts">
import { computed, onMounted, ref } from "vue";
import type { KeychainStatus, PassphraseMemory } from "@/lib/bindings";
import { Tip } from "@/components/ui/tooltip";
import { keychainStatus } from "@/lib/keys";

/**
 * Where a key's passphrase is remembered: not at all, until StashPanda closes, or in the system
 * keychain. Shared by the passphrase dialog and the Generate dialog. The default slot holds a
 * note of the caller's own, under the choices.
 */
const props = withDefaults(
  defineProps<{
    /** The name of the radio group: unique among the forms that can be open together. */
    name: string;
    legend?: string;
    /** Whether "not at all" can be chosen. A connection that reconnects by itself cannot do without. */
    allowNone?: boolean;
    /** How "not at all" reads. */
    noneLabel?: string;
    disabled?: boolean;
  }>(),
  { legend: "Remember it", allowNone: true, noneLabel: "Only this time" },
);

const memory = defineModel<PassphraseMemory>({ required: true });

const keychain = ref<KeychainStatus | null>(null);
onMounted(() => {
  keychainStatus()
    .then((status) => (keychain.value = status))
    .catch(() => (keychain.value = null));
});

const choices = computed(() => {
  const list: { value: PassphraseMemory; label: string; disabled?: boolean }[] = [];
  if (props.allowNone) list.push({ value: "none", label: props.noneLabel });
  list.push({ value: "session", label: "Until StashPanda closes" });
  list.push({
    value: "keychain",
    label: keychain.value ? `In ${keychain.value.name}, so it is remembered after a restart` : "In the system keychain",
    disabled: !(keychain.value?.available ?? false),
  });
  return list;
});
</script>

<template>
  <fieldset class="grid gap-1" :disabled="disabled">
    <legend class="mb-1 text-base">{{ legend }}</legend>
    <label v-for="choice in choices" :key="choice.value" class="flex items-center gap-2 text-base select-none" :class="choice.disabled && 'text-muted-foreground'">
      <input v-model="memory" type="radio" :name="name" class="size-3.5 accent-primary" :value="choice.value" :disabled="choice.disabled" />
      {{ choice.label }}
    </label>
    <!-- The keychain's own error is for diagnosing, so it is the tooltip rather than the text. -->
    <Tip v-if="keychain && !keychain.available" :content="keychain.detail">
      <p class="text-xs text-muted-foreground">No system keychain is available here, so a passphrase can only be remembered until StashPanda closes.</p>
    </Tip>
    <slot />
  </fieldset>
</template>
