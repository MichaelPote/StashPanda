<script setup lang="ts">
import { computed, ref } from "vue";
import { commands, type GraphRow, type RefKind } from "@/lib/bindings";
import { unwrap } from "@/lib/ipc";
import { ContextMenu, ContextMenuContent, ContextMenuSeparator, ContextMenuTrigger } from "@/components/ui/context-menu";
import HueSubmenu from "@/components/ui/HueSubmenu.vue";
import ActionMenuItem from "@/components/graph/ActionMenuItem.vue";
import CommitMenuItems from "@/components/graph/CommitMenuItems.vue";
import LaneBranchName from "@/components/graph/LaneBranchName.vue";
import {
  checkoutTargetOf,
  createBranchHere,
  currentBranch,
  deleteBranch,
  deleteRemoteBranch,
  deleteRemoteBranchReason,
  operationReason,
  fastForwardReason,
  fastForwardToHead,
  fastForwardToUpstream,
  isDetached,
  mergeInto,
  moveTag,
  pushBranch,
  pushBranchTo,
  pushBranchReason,
  pushTags,
  pushTagsReason,
  renameBranch,
  renameTag,
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
  /**
   * The commit's row, for a pill in the graph; it adds Cherry-pick, Revert and Reset. The Branches
   * panel passes none, since its branch may not be among the loaded rows.
   */
  row?: GraphRow | null;
}>();

const sessions = useSessionsStore();
const session = computed(() => sessions.sessions.get(props.sessionKey) ?? null);

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
/** The branch a checkout lands on, for its lane colour; null for a tag, which detaches HEAD. */
const checkoutFullName = computed(() => {
  if (props.kind.type === "remoteBranch") return `refs/heads/${checkoutTarget.value}`;
  return props.kind.type === "localBranch" ? props.fullName : null;
});
/** The checked-out branch's full name, for its lane colour. */
const branchFullName = computed(() => (branch.value ? `refs/heads/${branch.value}` : ""));

function runCheckout(): void {
  void requestCheckout(props.sessionKey, checkoutTargetOf(props));
}
// A dirty working tree does not block a checkout: requestCheckout asks first.
const checkoutReason = computed(() => (props.isCurrent ? `${props.short} is already checked out.` : null));

const mergeReason = computed(() => {
  if (props.isCurrent) return `${props.short} is the current branch.`;
  if (isDetached(session.value)) return "HEAD is detached. Check out a branch to merge into first.";
  if (session.value?.status?.headOid === props.target) return `${props.short} points at HEAD already.`;
  // Uncommitted changes do not block it: mergeInto asks, then stashes them around the merge.
  return operationReason(session.value);
});

const deleteReason = computed(() => (props.isCurrent ? "The checked-out branch cannot be deleted." : null));

/** The remotes a tag can be pushed to: one menu item each, or one greyed-out item when there are none. */
const tagRemotes = computed(() => session.value?.refs?.remotes.map((r) => r.name) ?? []);
const pushTagReason = computed(() => pushTagsReason(session.value));

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
      <!-- Labels naming a branch are one <span>, because the item is a flex row whose gap would replace the spaces between words. -->
      <ActionMenuItem :reason="checkoutReason" @select="runCheckout">
        <span>
          Checkout
          <LaneBranchName v-if="checkoutFullName" :session="session" :full-name="checkoutFullName" :name="checkoutTarget" />
          <template v-else>{{ checkoutTarget }}</template>
          <template v-if="createsLocal">(tracking <LaneBranchName :session="session" :full-name="fullName" :name="short" />)…</template>
        </span>
      </ActionMenuItem>
      <ActionMenuItem v-if="kind.type !== 'tag'" :reason="mergeReason" @select="mergeInto(sessionKey, revisionOf(props, session?.refs?.refs ?? []), short)">
        <span>
          Merge <LaneBranchName :session="session" :full-name="fullName" :name="short" /> into
          <LaneBranchName v-if="branch" :session="session" :full-name="branchFullName" :name="branch" />
          <template v-else>current branch</template>
        </span>
      </ActionMenuItem>
      <ActionMenuItem v-if="fastForwards && branch" :reason="fastForwardBlocked" @select="fastForwardToHead(sessionKey, short)">
        <span>Fast-forward to <LaneBranchName :session="session" :full-name="branchFullName" :name="branch" /></span>
      </ActionMenuItem>
      <ActionMenuItem v-if="upstreamTarget" :reason="fastForwardBlocked" @select="fastForwardToUpstream(sessionKey, short, upstreamTarget)">
        <span>Fast-forward to <LaneBranchName :session="session" :full-name="`refs/remotes/${upstreamTarget}`" :name="upstreamTarget" /></span>
      </ActionMenuItem>
      <ActionMenuItem @select="createBranchHere(sessionKey, target, short)">Create Branch Here…</ActionMenuItem>

      <template v-if="kind.type === 'localBranch'">
        <ContextMenuSeparator />
        <ActionMenuItem :reason="pushReason" @select="pushBranch(session, short)">
          <span v-if="pushUpstream">Push to <LaneBranchName :session="session" :full-name="`refs/remotes/${pushUpstream}`" :name="pushUpstream" /></span>
          <template v-else>Push to a New Remote Branch…</template>
        </ActionMenuItem>
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
        <ActionMenuItem v-if="tagRemotes.length === 0" :reason="pushTagReason">Push Tag</ActionMenuItem>
        <ActionMenuItem
          v-for="remote in tagRemotes"
          :key="remote"
          :reason="pushTagReason"
          @select="pushTags(sessionKey, remote, { type: 'named', names: [short] }, short)"
        >
          Push Tag to {{ remote }}
        </ActionMenuItem>
        <ActionMenuItem @select="renameTag(sessionKey, short)">Rename Tag…</ActionMenuItem>
        <ActionMenuItem @select="moveTag(sessionKey, short)">Move Tag…</ActionMenuItem>
      </template>

      <ContextMenuSeparator />
      <!-- Kept in the app's own UI state, not in the repository. -->
      <HueSubmenu :chosen="chosenColour" @choose="chooseColour" />
      <ContextMenuSeparator />
      <!-- The commit row's items, from Cherry-pick on; Remove Tag for a tag is among them. -->
      <CommitMenuItems v-if="session" :session="session" :oid="target" :row="row" />
    </ContextMenuContent>
  </ContextMenu>
</template>
