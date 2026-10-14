<script setup lang="ts">
import { computed } from "vue";
import { CopyIcon, EllipsisVerticalIcon, KeyRoundIcon, LockIcon, TriangleAlertIcon } from "@lucide/vue";
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
  | "delete";

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
const isOpenSsh = computed(() => props.info.format === "openSsh");
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
      <span class="min-w-0 truncate text-base font-medium" :title="info.path">{{ info.fileName }}</span>
      <Badge variant="outline">{{ keyFormatLabel(info.format) }}</Badge>
      <Badge v-if="info.encrypted" variant="secondary"><LockIcon />Encrypted</Badge>
      <Badge v-if="info.inAgent" variant="secondary">In agent</Badge>
      <Badge v-if="rememberedLabel" variant="secondary">{{ rememberedLabel }}</Badge>
      <Badge v-if="isDeprecatedKey(info)" variant="destructive" title="OpenSSH 9 and later refuse DSA keys. Generate a new key to replace it.">
        <TriangleAlertIcon />Deprecated
      </Badge>
      <div class="ml-auto flex shrink-0 items-center gap-1">
        <Button type="button" variant="outline" size="sm" :disabled="busy || info.error !== null" @click="emit('action', 'copyPublic')">
          <CopyIcon />
          Copy public key
        </Button>
        <DropdownMenu>
          <DropdownMenuTrigger as-child>
            <Button type="button" variant="ghost" size="icon-sm" :disabled="busy" :aria-label="`Actions for ${info.fileName}`" title="More actions">
              <EllipsisVerticalIcon />
            </Button>
          </DropdownMenuTrigger>
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
            <DropdownMenuItem :disabled="isOpenSsh || info.error !== null" @select="emit('action', 'convert')">
              Convert to OpenSSH
            </DropdownMenuItem>
            <DropdownMenuItem :disabled="info.error !== null" @select="emit('action', 'export')">Export…</DropdownMenuItem>
            <DropdownMenuSeparator />
            <DropdownMenuItem :disabled="!agentAvailable || info.inAgent || info.error !== null" @select="emit('action', 'addToAgent')">
              Add to agent
            </DropdownMenuItem>
            <DropdownMenuItem :disabled="!agentAvailable || info.inAgent || info.error !== null" @select="emit('action', 'addToAgentForHours')">
              Add to agent for 8 hours
            </DropdownMenuItem>
            <DropdownMenuSeparator />
            <DropdownMenuItem :disabled="!isOpenSsh || info.error !== null" @select="emit('action', 'changePassphrase')">
              Change passphrase…
            </DropdownMenuItem>
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
      <span v-if="info.comment" class="truncate" :title="info.comment">{{ info.comment }}</span>
    </div>
    <div class="flex min-w-0 flex-wrap gap-x-3 gap-y-0.5 text-xs text-muted-foreground">
      <span class="min-w-0 truncate font-mono" :title="info.path">{{ info.path }}</span>
      <span v-if="info.publicKeyFile">Public key file: {{ pathBasename(info.publicKeyFile) }}</span>
      <span v-else>No .pub file beside it</span>
      <span v-if="!isOpenSsh && info.error === null" title="Change passphrase only rewrites OpenSSH files; convert it first.">
        Convert to OpenSSH to change its passphrase
      </span>
    </div>
    <p v-if="usedBy.length > 0" class="text-xs text-muted-foreground">Used by {{ usedBy.join(", ") }}</p>
    <p v-if="info.error" class="text-xs break-words text-destructive">{{ info.error }}</p>
  </div>
</template>
