<script setup lang="ts">
import { computed } from "vue";
import type { RefKind } from "@/lib/bindings";
import { ContextMenu, ContextMenuContent, ContextMenuSeparator, ContextMenuTrigger } from "@/components/ui/context-menu";
import HueSubmenu from "@/components/ui/HueSubmenu.vue";
import ActionMenuItem from "@/components/graph/ActionMenuItem.vue";
import { shortSha } from "@/lib/refNames";
import {
  copyText,
  createBranchHere,
  currentBranch,
  deleteBranch,
  dirtyTreeReason,
  isDetached,
  mergeInto,
  removeTag,
  renameBranch,
  requestCheckout,
  stripRemote,
} from "@/lib/repoActions";
import { useSessionsStore } from "@/stores/sessions";

/**
 * Right-click menu for one branch or tag, used by the Branches panel and by the ref pills in the
 * graph. The slot is the element that becomes the trigger. The `contextmenu` event stops here,
 * so a pill inside a commit row does not also open the commit's menu.
 */
const props = defineProps<{
  sessionKey: string;
  fullName: string;
  /** `dev`, `origin/dev` or `v1.2` */
  short: string;
  kind: RefKind;
  /** Commit the ref points at. */
  target: string;
  /** True for the checked-out local branch. */
  isCurrent: boolean;
}>();

const sessions = useSessionsStore();
const session = computed(() => sessions.sessions.get(props.sessionKey) ?? null);

const dirty = computed(() => dirtyTreeReason(session.value));
const branch = computed(() => currentBranch(session.value));

/**
 * For a remote branch the checkout target is the name without the remote (`dev`, not
 * `origin/dev`): `git checkout dev` creates a local branch tracking `origin/dev` when no local
 * `dev` exists, whereas `git checkout origin/dev` would only detach HEAD at that commit.
 */
const checkoutTarget = computed(() => (props.kind.type === "remoteBranch" ? stripRemote(props.short, props.kind.remote) : props.short));
/** A remote branch with no local branch of its name: checking it out creates one, after a confirm. */
const createsLocal = computed(
  () =>
    props.kind.type === "remoteBranch" &&
    !(session.value?.refs?.refs ?? []).some((r) => r.kind.type === "localBranch" && r.short === checkoutTarget.value),
);
const checkoutText = computed(() => {
  if (createsLocal.value) return `Checkout ${checkoutTarget.value} (tracking ${props.short})…`;
  return `Checkout ${checkoutTarget.value}`;
});

function runCheckout(): void {
  if (props.kind.type === "remoteBranch") {
    void requestCheckout(props.sessionKey, { kind: "remote", remote: props.short, local: checkoutTarget.value });
  } else if (props.kind.type === "localBranch") {
    void requestCheckout(props.sessionKey, { kind: "local", name: props.short });
  } else {
    void requestCheckout(props.sessionKey, { kind: "revision", revision: props.short, label: props.short });
  }
}
// A dirty working tree does not block a checkout: requestCheckout asks first.
const checkoutReason = computed(() => (props.isCurrent ? `${props.short} is already checked out.` : null));

const mergeReason = computed(() => {
  if (props.isCurrent) return `${props.short} is the current branch.`;
  if (isDetached(session.value)) return "HEAD is detached. Check out a branch to merge into first.";
  if (session.value?.status?.headOid === props.target) return `${props.short} points at HEAD already.`;
  return dirty.value;
});

const deleteReason = computed(() => (props.isCurrent ? "The checked-out branch cannot be deleted." : null));

/** The colour chosen for this ref, or null for the automatic one. */
const chosenColour = computed(() => session.value?.refColours[props.fullName] ?? null);

function chooseColour(colour: number | null): void {
  void sessions.setRefColour(props.sessionKey, props.fullName, colour);
}
</script>

<template>
  <ContextMenu>
    <ContextMenuTrigger as-child @contextmenu.stop>
      <slot />
    </ContextMenuTrigger>
    <ContextMenuContent class="min-w-52">
      <ActionMenuItem :reason="checkoutReason" @select="runCheckout">
        {{ checkoutText }}
      </ActionMenuItem>
      <ActionMenuItem v-if="kind.type !== 'tag'" :reason="mergeReason" @select="mergeInto(sessionKey, short)">
        Merge {{ short }} into {{ branch ?? "current branch" }}
      </ActionMenuItem>
      <ActionMenuItem @select="createBranchHere(sessionKey, target, short)">Create Branch Here…</ActionMenuItem>

      <template v-if="kind.type === 'localBranch'">
        <ContextMenuSeparator />
        <ActionMenuItem @select="renameBranch(sessionKey, short)">Rename Branch…</ActionMenuItem>
        <ActionMenuItem destructive :reason="deleteReason" @select="deleteBranch(sessionKey, short)">Delete Branch…</ActionMenuItem>
      </template>
      <template v-else-if="kind.type === 'tag'">
        <ContextMenuSeparator />
        <ActionMenuItem destructive @select="removeTag(sessionKey, short)">Remove Tag {{ short }}</ActionMenuItem>
      </template>

      <ContextMenuSeparator />
      <!-- Kept in the app's own UI state, not in the repository. -->
      <HueSubmenu :chosen="chosenColour" @choose="chooseColour" />
      <ContextMenuSeparator />
      <ActionMenuItem @select="copyText(target, 'the SHA')">Copy SHA {{ shortSha(target) }}</ActionMenuItem>
    </ContextMenuContent>
  </ContextMenu>
</template>
