<script setup lang="ts">
import { computed } from "vue";
import { CopyIcon, CopyPlusIcon, EllipsisVerticalIcon, KeyRoundIcon, LockIcon, TriangleAlertIcon } from "@lucide/vue";
import type { KeyInfo, RepoEntry } from "@/lib/bindings";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuSub,
  DropdownMenuSubContent,
  DropdownMenuSubTrigger,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Tip } from "@/components/ui/tooltip";
import { pathBasename } from "@/lib/format";
import { isDeprecatedKey, keyAlgorithmLabel, keyFormatLabel } from "@/lib/keys";
import { repoDisplayName } from "@/stores/library";

/** What the page does for a row; the page holds the logic, the row only asks. */
export type KeyAction =
  | "convert"
  | "export"
  | "copyPublic"
  | "addToAgent"
  | "addToAgentForHours"
  | "changePassphrase"
  | "forgetPassphrase"
  | "reveal"
  | "delete"
  | "copyTo";

const props = defineProps<{
  info: KeyInfo;
  /** Library entries reached over SSH, for "Use for repository". */
  sshRepos: RepoEntry[];
  /** False when no SSH agent answered, so "Add to agent" cannot work. */
  agentAvailable: boolean;
  /** An action on this key is running. */
  busy: boolean;
}>();

const emit = defineEmits<{
  action: [kind: KeyAction];
  useFor: [entry: RepoEntry];
}>();

const algorithm = computed(() => keyAlgorithmLabel(props.info));
/** A key on a server, read through a repository's connection: it can only be copied from there. */
const remote = computed(() => props.info.place.type === "remote");
const isOpenSsh = computed(() => props.info.format === "openSsh");

// Why each greyed-out action is unavailable, shown as its tooltip.
const unreadable = computed(() => (props.info.error !== null ? "StashPanda cannot read this key." : null));
const convertReason = computed(() => unreadable.value ?? (isOpenSsh.value ? "The key is in OpenSSH format already." : null));
const agentReason = computed(
  () =>
    unreadable.value ??
    (!props.agentAvailable ? "No SSH agent is running, or StashPanda cannot reach it." : props.info.inAgent ? "The agent has this key already." : null),
);
const passphraseReason = computed(() => unreadable.value ?? (!isOpenSsh.value ? "Convert the key to OpenSSH first." : null));
/** shadcn's item turns pointer events off when disabled, which would hide the tooltip too. */
const REASON_CLASS = "data-disabled:pointer-events-auto";
/** Repositories already set to log in with this key. */
const usedBy = computed(() =>
  props.sshRepos.filter((r) => r.connection?.type === "ssh" && r.connection.identityFile === props.info.path).map(repoDisplayName),
);

const rememberedLabel = computed(() => {
  switch (props.info.passphraseRemembered) {
    case "keychain":
      return "Passphrase in keychain";
    case "session":
      return "Passphrase remembered";
    default:
      return null;
  }
});
</script>

<template>
  <div class="grid gap-1 rounded-lg border border-border px-3 py-2" :aria-busy="busy || undefined">
    <div class="flex min-w-0 items-center gap-2">
      <KeyRoundIcon class="size-4 shrink-0 text-muted-foreground" />
      <span class="min-w-0 truncate text-base font-medium">{{ info.fileName }}</span>
      <Badge variant="outline">{{ keyFormatLabel(info.format) }}</Badge>
      <Badge v-if="info.encrypted" variant="secondary"><LockIcon />Encrypted</Badge>
      <Badge v-if="info.inAgent" variant="secondary">In agent</Badge>
      <Badge v-if="rememberedLabel" variant="secondary">{{ rememberedLabel }}</Badge>
      <Tip v-if="isDeprecatedKey(info)" content="OpenSSH 9 and later refuse DSA keys. Generate a new key to replace it.">
        <Badge variant="destructive">
          <TriangleAlertIcon />Deprecated
        </Badge>
      </Tip>
      <div class="ml-auto flex shrink-0 items-center gap-1">
        <Tip content="Copy the key, and its .pub file, to Windows, a WSL distro or a server. An existing key is never replaced.">
          <Button type="button" variant="outline" size="sm" :disabled="busy" @click="emit('action', 'copyTo')">
            <CopyPlusIcon />
            Copy to…
          </Button>
        </Tip>
        <Button v-if="!remote" type="button" variant="outline" size="sm" :disabled="busy || info.error !== null" @click="emit('action', 'copyPublic')">
          <CopyIcon />
          Copy public key
        </Button>
        <DropdownMenu v-if="!remote">
          <Tip content="More actions">
            <DropdownMenuTrigger as-child>
              <Button type="button" variant="ghost" size="icon-sm" :disabled="busy" :aria-label="`Actions for ${info.fileName}`">
                <EllipsisVerticalIcon />
              </Button>
            </DropdownMenuTrigger>
          </Tip>
          <DropdownMenuContent align="end" class="min-w-56">
            <DropdownMenuSub>
              <DropdownMenuSubTrigger :disabled="info.error !== null">Use for repository</DropdownMenuSubTrigger>
              <DropdownMenuSubContent class="max-w-80">
                <DropdownMenuItem v-if="sshRepos.length === 0" disabled>No SSH repositories in the library</DropdownMenuItem>
                <DropdownMenuItem v-for="repo in sshRepos" :key="repo.key ?? repo.path" @select="emit('useFor', repo)">
                  <span class="truncate">{{ repoDisplayName(repo) }}</span>
                </DropdownMenuItem>
              </DropdownMenuSubContent>
            </DropdownMenuSub>
            <Tip :content="convertReason" side="right">
              <DropdownMenuItem :disabled="convertReason !== null" :class="REASON_CLASS" @select="emit('action', 'convert')">
                Convert to OpenSSH
              </DropdownMenuItem>
            </Tip>
            <DropdownMenuItem :disabled="info.error !== null" @select="emit('action', 'export')">Export…</DropdownMenuItem>
            <DropdownMenuSeparator />
            <Tip :content="agentReason" side="right">
              <DropdownMenuItem :disabled="agentReason !== null" :class="REASON_CLASS" @select="emit('action', 'addToAgent')">
                Add to agent
              </DropdownMenuItem>
            </Tip>
            <Tip :content="agentReason" side="right">
              <DropdownMenuItem :disabled="agentReason !== null" :class="REASON_CLASS" @select="emit('action', 'addToAgentForHours')">
                Add to agent for 8 hours
              </DropdownMenuItem>
            </Tip>
            <DropdownMenuSeparator />
            <Tip :content="passphraseReason" side="right">
              <DropdownMenuItem :disabled="passphraseReason !== null" :class="REASON_CLASS" @select="emit('action', 'changePassphrase')">
                Change passphrase…
              </DropdownMenuItem>
            </Tip>
            <DropdownMenuItem v-if="info.passphraseRemembered !== 'none'" @select="emit('action', 'forgetPassphrase')">
              Forget passphrase
            </DropdownMenuItem>
            <DropdownMenuItem @select="emit('action', 'reveal')">Reveal in folder</DropdownMenuItem>
            <DropdownMenuSeparator />
            <DropdownMenuItem variant="destructive" @select="emit('action', 'delete')">Delete…</DropdownMenuItem>
          </DropdownMenuContent>
        </DropdownMenu>
      </div>
    </div>

    <div class="flex min-w-0 flex-wrap items-baseline gap-x-3 gap-y-0.5 text-xs text-muted-foreground">
      <span v-if="algorithm" class="text-foreground">{{ algorithm }}</span>
      <span v-if="info.fingerprint" class="font-mono select-text">{{ info.fingerprint }}</span>
      <span v-else-if="info.error === null">The fingerprint needs the passphrase: there is no .pub file beside it.</span>
      <Tip v-if="info.comment" :content="info.comment">
        <span class="truncate">{{ info.comment }}</span>
      </Tip>
    </div>
    <div class="flex min-w-0 flex-wrap gap-x-3 gap-y-0.5 text-xs text-muted-foreground">
      <Tip :content="info.path">
        <span class="min-w-0 truncate font-mono">{{ info.path }}</span>
      </Tip>
      <span v-if="info.publicKeyFile">Public key file: {{ pathBasename(info.publicKeyFile) }}</span>
      <span v-else>No .pub file beside it</span>
      <span v-if="!isOpenSsh && info.error === null">Convert to OpenSSH to change its passphrase</span>
    </div>
    <p v-if="usedBy.length > 0" class="text-xs text-muted-foreground">Used by {{ usedBy.join(", ") }}</p>
    <p v-if="info.error" class="text-xs break-words text-destructive">{{ info.error }}</p>
  </div>
</template>
