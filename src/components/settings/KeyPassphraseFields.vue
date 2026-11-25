<script setup lang="ts">
import { computed } from "vue";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";

/**
 * A new passphrase, typed twice, for the Generate form and the Export and Change Passphrase
 * dialogs. Says when the two differ; the form that holds it compares them again before it submits.
 */
defineProps<{
  /** Starts the two fields' ids, so that two forms open at once do not share them. */
  idPrefix: string;
  /** Marks the passphrase as optional: Generate writes an unencrypted key when it is left empty. */
  optional?: boolean;
  disabled?: boolean;
}>();

const passphrase = defineModel<string>("passphrase", { required: true });
const again = defineModel<string>("again", { required: true });

const mismatch = computed(() => passphrase.value !== again.value);
</script>

<template>
  <div class="grid grid-cols-2 gap-3">
    <div class="grid gap-1.5">
      <Label :for="`${idPrefix}-passphrase`" class="text-base">
        Passphrase <span v-if="optional" class="text-muted-foreground">(optional)</span>
      </Label>
      <Input :id="`${idPrefix}-passphrase`" v-model="passphrase" type="password" class="text-base" autocomplete="new-password" :disabled="disabled" />
    </div>
    <div class="grid gap-1.5">
      <Label :for="`${idPrefix}-passphrase-again`" class="text-base">Passphrase again</Label>
      <Input
        :id="`${idPrefix}-passphrase-again`"
        v-model="again"
        type="password"
        class="text-base"
        autocomplete="new-password"
        :disabled="disabled"
        :aria-invalid="mismatch || undefined"
      />
    </div>
    <p v-if="mismatch && again.length > 0" class="col-span-2 text-xs text-destructive">The two passphrases differ.</p>
  </div>
</template>
