<script setup lang="ts">
import { computed, onBeforeUnmount, reactive, ref, watch } from "vue";
import { busyReason } from "@/lib/repoActions";
import { checkRemoteTags, remoteTagLabel, remoteTagNote, remoteTagTickable, type RemoteTagAction, type RemoteTagState } from "@/lib/remoteTags";
import type { RemoteTagChoice } from "@/stores/dialogs";
import { useSessionsStore } from "@/stores/sessions";

/**
 * One checkbox for each remote in Remove Tag, Rename Tag and Move Tag, unticked to start with.
 * When it mounts it asks every remote in the background whether it has the tag, and the line
 * under each checkbox says what came back. The model is the ticked remotes, each with what was
 * found there.
 */
const props = defineProps<{
  sessionKey: string;
  action: RemoteTagAction;
  /** The tag's name. */
  tag: string;
  /** The commit the tag is on here. */
  commit: string;
  /** Rename Tag's new name, for the notes. */
  renamed?: string;
}>();

const chosen = defineModel<RemoteTagChoice[]>({ required: true });

const sessions = useSessionsStore();
const session = computed(() => sessions.sessions.get(props.sessionKey) ?? null);
const remotes = computed(() => session.value?.refs?.remotes.map((r) => r.name) ?? []);
/** A fetch, pull or push already running: the remote steps would wait for it, so they are not offered. */
const busy = computed(() => (session.value ? busyReason(session.value) : null));

const states = reactive<Record<string, RemoteTagState>>({});
const ticked = ref<string[]>([]);

for (const remote of remotes.value) states[remote] = { state: "checking" };
const stop = checkRemoteTags(props.sessionKey, props.tag, remotes.value, (remote, state) => {
  states[remote] = state;
  if (!remoteTagTickable(props.action, state)) ticked.value = ticked.value.filter((r) => r !== remote);
});
onBeforeUnmount(stop);

function stateOf(remote: string): RemoteTagState {
  return states[remote] ?? { state: "checking" };
}

watch(
  [ticked, states],
  () => {
    chosen.value = ticked.value.map((remote) => {
      const s = stateOf(remote);
      return { remote, found: s.state === "found" ? s.tag : s.state === "missing" ? null : undefined };
    });
  },
  { deep: true, immediate: true },
);
</script>

<template>
  <div v-if="remotes.length > 0" class="grid gap-2">
    <div v-for="remote in remotes" :key="remote" class="grid gap-0.5">
      <label class="flex items-center gap-2 text-base select-none" :class="{ 'opacity-60': busy || !remoteTagTickable(action, stateOf(remote)) }">
        <input
          v-model="ticked"
          type="checkbox"
          class="size-3.5 accent-primary"
          :value="remote"
          :disabled="busy !== null || !remoteTagTickable(action, stateOf(remote))"
          :data-remote-tag-option="remote"
        />
        {{ remoteTagLabel(action, remote) }}
      </label>
      <p class="pl-5.5 text-xs text-muted-foreground">{{ remoteTagNote(action, remote, stateOf(remote), tag, commit, renamed) }}</p>
    </div>
    <p v-if="busy" class="text-xs text-muted-foreground">{{ busy }}</p>
  </div>
</template>
