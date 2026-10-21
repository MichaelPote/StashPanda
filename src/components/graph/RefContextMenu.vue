<script setup lang="ts">
import { computed, ref } from "vue";
import { commands, type RefKind } from "@/lib/bindings";
import { unwrap } from "@/lib/ipc";
import { ContextMenu, ContextMenuContent, ContextMenuSeparator, ContextMenuTrigger } from "@/components/ui/context-menu";
import HueSubmenu from "@/components/ui/HueSubmenu.vue";
import ActionMenuItem from "@/components/graph/ActionMenuItem.vue";
import { shortSha } from "@/lib/refNames";
import {
  checkoutTargetOf,
  copyText,
  createBranchHere,
  currentBranch,
  deleteBranch,
  deleteRemoteBranch,
  deleteRemoteBranchReason,
  dirtyTreeReason,
  fastForwardReason,
  fastForwardToHead,
  fastForwardToUpstream,
  isDetached,
  mergeInto,
  pushBranch,
  pushBranchTo,
  pushBranchReason,
  removeTag,
  renameBranch,
  requestCheckout,
  revisionOf,
  stripRemote,
  upstreamFastForward,
  trackRemoteBranch,
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
  void requestCheckout(props.sessionKey, checkoutTargetOf(props));
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

/**
 * True when this local branch is behind HEAD on the checked-out branch's own lane, so it can be
 * fast-forwarded to it. Asked each time the menu opens; the item shows once the answer is yes.
 */
const fastForwards = ref(false);
const fastForwardBlocked = computed(() => fastForwardReason(session.value));

/**
 * The remote branch this local branch can be fast-forwarded to (`origin/main`), checked out or
 * not, from the refs as last fetched; null hides the item. Uncommitted changes do not block it:
 * for the checked-out branch, the action asks and stashes them around the fast-forward.
 */
const upstreamTarget = computed(() => {
  const entry = (session.value?.refs?.refs ?? []).find((r) => r.fullName === props.fullName);
  return entry ? upstreamFastForward(entry) : null;
});
async function onOpenChange(open: boolean): Promise<void> {
  if (!open) return;
  fastForwards.value = false;
  if (props.kind.type !== "localBranch" || props.isCurrent || branch.value === null) return;
  try {
    fastForwards.value = await unwrap(commands.getFastForwardsToHead(props.sessionKey, props.short));
  } catch {
    // Not offered.
  }
}

/** A local branch pushes to the remote branch it tracks; with none, the Push dialog creates one. */
const pushUpstream = computed(() => {
  if (props.kind.type !== "localBranch") return null;
  const up = (session.value?.refs?.refs ?? []).find((r) => r.fullName === props.fullName)?.upstream ?? null;
  return up?.startsWith("refs/remotes/") ? up.slice("refs/remotes/".length) : null;
});
const pushText = computed(() => (pushUpstream.value ? `Push to ${pushUpstream.value}` : "Push to a New Remote Branch…"));
const pushReason = computed(() => pushBranchReason(session.value, props.short));
/** A fetch, pull or push already running would have to finish first. */
const deleteRemoteReason = computed(() => deleteRemoteBranchReason(session.value));

/** The remote of a remote branch, for the delete; empty for any other kind of ref. */
const remoteName = computed(() => (props.kind.type === "remoteBranch" ? props.kind.remote : ""));

/** The colour chosen for this ref, or null for the automatic one. */
const chosenColour = computed(() => session.value?.refColours[props.fullName] ?? null);

function chooseColour(colour: number | null): void {
  void sessions.setRefColour(props.sessionKey, props.fullName, colour);
}
</script>

<template>
  <ContextMenu @update:open="onOpenChange">
    <ContextMenuTrigger as-child @contextmenu.stop>
      <slot />
    </ContextMenuTrigger>
    <ContextMenuContent class="min-w-52">
      <ActionMenuItem :reason="checkoutReason" @select="runCheckout">
        {{ checkoutText }}
      </ActionMenuItem>
      <ActionMenuItem v-if="kind.type !== 'tag'" :reason="mergeReason" @select="mergeInto(sessionKey, revisionOf(props, session?.refs?.refs ?? []), short)">
        Merge {{ short }} into {{ branch ?? "current branch" }}
      </ActionMenuItem>
      <ActionMenuItem v-if="fastForwards && branch" :reason="fastForwardBlocked" @select="fastForwardToHead(sessionKey, short)">
        Fast-forward to {{ branch }}
      </ActionMenuItem>
      <ActionMenuItem v-if="upstreamTarget" :reason="fastForwardBlocked" @select="fastForwardToUpstream(sessionKey, short, upstreamTarget)">
        Fast-forward to {{ upstreamTarget }}
      </ActionMenuItem>
      <ActionMenuItem @select="createBranchHere(sessionKey, target, short)">Create Branch Here…</ActionMenuItem>

      <template v-if="kind.type === 'localBranch'">
        <ContextMenuSeparator />
        <ActionMenuItem :reason="pushReason" @select="pushBranch(session, short)">{{ pushText }}</ActionMenuItem>
        <ActionMenuItem :reason="fastForwardBlocked" @select="pushBranchTo(sessionKey, short)">Push To…</ActionMenuItem>
        <ActionMenuItem @select="trackRemoteBranch(sessionKey, short)">Track Remote Branch…</ActionMenuItem>
        <ActionMenuItem @select="renameBranch(sessionKey, short)">Rename Branch…</ActionMenuItem>
        <ActionMenuItem destructive :reason="deleteReason" @select="deleteBranch(sessionKey, short)">Delete Branch…</ActionMenuItem>
      </template>
      <template v-else-if="kind.type === 'remoteBranch'">
        <ContextMenuSeparator />
        <ActionMenuItem destructive :reason="deleteRemoteReason" @select="deleteRemoteBranch(sessionKey, short, remoteName)">
          Delete Remote Branch…
        </ActionMenuItem>
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
