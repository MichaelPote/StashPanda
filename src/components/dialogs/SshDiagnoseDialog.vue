<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { CopyIcon, LoaderCircleIcon } from "@lucide/vue";
import { commands, type IdentityFile, type KeyLine, type SshSetup, type SshTest } from "@/lib/bindings";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Tip } from "@/components/ui/tooltip";
import { errorMessage, unwrap } from "@/lib/ipc";
import { copyText } from "@/lib/repoActions";
import { cn } from "@/lib/utils";
import { useDialogsStore } from "@/stores/dialogs";
import { useSessionsStore } from "@/stores/sessions";

/**
 * Opened from the Diagnose… button on a notice, when a remote refused every SSH key Git offered.
 * Shows which Git and which ssh the repository uses, the key files and agent keys ssh has, and
 * runs a login test with the same ssh, all in the place Git runs: Windows, a WSL distro or the
 * server of a repository opened over SSH.
 */
const dialogs = useDialogsStore();
const sessions = useSessionsStore();

const request = computed(() => dialogs.current("sshDiagnose"));

const setup = ref<SshSetup | null>(null);
const setupError = ref<string | null>(null);
const test = ref<SshTest | null>(null);
const testError = ref<string | null>(null);
const testing = ref(false);
/** Bumped by each load, so a reply for an earlier request is dropped. */
let serial = 0;

watch(request, (r) => {
  setup.value = null;
  setupError.value = null;
  test.value = null;
  testError.value = null;
  if (r) {
    void loadSetup();
    void runTest();
  }
});

async function loadSetup(): Promise<void> {
  const r = request.value;
  if (!r) return;
  const mine = ++serial;
  try {
    const result = await unwrap(commands.remoteSshSetup(r.key, r.dest, r.port));
    if (mine === serial) setup.value = result;
  } catch (e) {
    if (mine === serial) setupError.value = errorMessage(e);
  }
}

async function runTest(): Promise<void> {
  const r = request.value;
  if (!r || testing.value) return;
  testing.value = true;
  testError.value = null;
  try {
    const result = await unwrap(commands.remoteSshTest(r.key, r.dest, r.port));
    if (request.value === r) test.value = result;
  } catch (e) {
    if (request.value === r) testError.value = errorMessage(e);
  } finally {
    testing.value = false;
  }
}

/** Where Git runs for this repository, as a phrase: "on Windows", "in WSL (Ubuntu)", "on build.example.com". */
const where = computed(() => {
  const r = request.value;
  const transport = r ? sessions.sessions.get(r.key)?.info.transport : undefined;
  switch (transport?.type) {
    case "wsl":
      return `in WSL (${transport.distro})`;
    case "ssh":
      return `on ${transport.host}`;
    case "local":
      return /\.exe$/i.test(transport.gitExecutable) ? "on Windows" : "on this computer";
    default:
      return "where Git runs";
  }
});

/** Where to paste the test command: Git for Windows' own ssh only runs as Git does in Git Bash. */
const runIn = computed(() => (where.value === "on Windows" && setup.value?.sshSource === "default" ? "in Git Bash" : where.value));

const SOURCE_TEXT: Record<string, string> = {
  default: "Git's default",
  "core.sshCommand": "set by core.sshCommand",
  GIT_SSH_COMMAND: "set by the GIT_SSH_COMMAND environment variable",
  GIT_SSH: "set by the GIT_SSH environment variable",
};

const agentKeys = computed<KeyLine[]>(() => (setup.value?.agent.state === "keys" ? setup.value.agent.keys : []));
const agentFingerprints = computed(() => new Set(agentKeys.value.map((k) => k.fingerprint)));

function triedFor(fingerprint: string | undefined | null): SshTest["keys"][number] | undefined {
  return fingerprint ? test.value?.keys.find((k) => k.fingerprint === fingerprint) : undefined;
}

const STATE_TEXT: Record<IdentityFile["state"], string> = {
  present: "Found",
  missing: "Not there",
  encrypted: "Encrypted",
  unreadable: "Unreadable",
};

function stateClass(f: IdentityFile): string {
  switch (f.state) {
    case "present":
      return "badge-added";
    case "missing":
      return "badge-neutral";
    default:
      return "badge-deleted";
  }
}

function keyText(k: KeyLine | null | undefined): string {
  if (!k) return "";
  const kind = [k.keyType, k.bits ? `${k.bits} bits` : ""].filter(Boolean).join(", ");
  return [k.fingerprint, kind && `(${kind})`, k.comment && k.comment !== "no comment" ? k.comment : ""].filter(Boolean).join(" ");
}

/** What the setup and the test add up to, in a sentence or two each. */
const findings = computed<string[]>(() => {
  const s = setup.value;
  const t = test.value;
  const out: string[] = [];
  if (!s || !t) return out;
  const host = s.hostname || request.value?.dest || "the server";
  const locked = s.identityFiles.filter((f) => f.state === "encrypted");
  const offered = t.keys.filter((k) => k.offered);
  const accepted = t.keys.find((k) => k.accepted);
  switch (t.outcome) {
    case "authenticated":
      out.push(
        `The test login worked${accepted ? ` with ${accepted.label}` : ""}. If Git still fails, the remote's URL may name a different user or host from ${request.value?.dest}.`,
      );
      break;
    case "denied":
      if (offered.length === 0) out.push(`ssh had no key it could offer to ${host}, so the server never saw one.`);
      else out.push(`${host} refused ${offered.length === 1 ? "the one key" : `all ${offered.length} keys`} ssh offered. Add the public key of the key you use to your account on ${host}.`);
      break;
    case "hostKeyUnverified":
      out.push(`ssh does not know ${host}'s host key ${where.value}, so it stopped before logging in. Run the command below once and accept the host key.`);
      break;
    case "couldNotConnect":
      out.push(`ssh could not reach ${host} ${where.value}. The output below says why.`);
      break;
    default:
      out.push("The test did not say whether the login worked. The output below has the details.");
  }
  if (t.outcome !== "authenticated") {
    if (locked.length > 0) {
      out.push(
        `${locked.length === 1 ? `${locked[0].path} is` : `${locked.length} key files are`} encrypted in the old PEM format with no .pub file beside ${locked.length === 1 ? "it" : "them"}. ssh needs the passphrase to use such a key, and nothing can type it when StashPanda runs Git, so it is skipped. Load it into the SSH agent, or convert it in Settings › SSH Keys.`,
      );
    }
    if (s.agent.state === "unavailable") out.push(`No SSH agent is reachable ${where.value}, so ssh only has the key files below.`);
    if (s.identitiesOnly && agentKeys.value.length > 0) {
      out.push("IdentitiesOnly is on for this host, so ssh offers only the key files below, not other keys in the agent.");
    }
  }
  return out;
});

const outcomeText = computed(() => {
  switch (test.value?.outcome) {
    case "authenticated":
      return "Logged in";
    case "denied":
      return "Refused";
    case "hostKeyUnverified":
      return "Unknown host key";
    case "couldNotConnect":
      return "Could not connect";
    default:
      return "Unclear";
  }
});

function onOpenChange(open: boolean): void {
  if (!open) dialogs.resolve("sshDiagnose", null);
}
</script>

<template>
  <Dialog :open="request !== null" @update:open="onOpenChange">
    <DialogContent v-if="request" class="sm:max-w-3xl" size-key="sshDiagnose">
      <DialogHeader>
        <DialogTitle>SSH Diagnosis for {{ request.dest }}</DialogTitle>
        <DialogDescription>
          {{ request.dest }} refused the SSH keys Git offered. This is the Git and the ssh that this repository uses {{ where }}, the keys ssh has
          there, and a test login with the same ssh.
        </DialogDescription>
      </DialogHeader>

      <div class="grid max-h-[60vh] min-w-0 gap-4 overflow-auto pr-1 text-base scrollbar-thin" data-dialog-grow style="--grow-basis: 60vh">
        <!-- What it adds up to. -->
        <section class="grid gap-1.5">
          <h3 class="font-semibold">Test login</h3>
          <div class="flex items-center gap-2">
            <LoaderCircleIcon v-if="testing" class="size-3.5 animate-spin" />
            <span v-if="testing" class="text-muted-foreground">Logging in to {{ request.dest }} {{ where }}…</span>
            <span
              v-else-if="test"
              :class="cn('rounded-sm px-1.5 text-xs font-semibold', test.outcome === 'authenticated' ? 'badge-added' : 'badge-deleted')"
              data-ssh-outcome
            >
              {{ outcomeText }}
            </span>
          </div>
          <p v-if="testError" class="text-destructive">{{ testError }}</p>
          <ul v-if="findings.length > 0" class="grid list-disc gap-1 pl-5">
            <li v-for="f in findings" :key="f">{{ f }}</li>
          </ul>
          <div v-if="test" class="grid gap-1">
            <p class="text-muted-foreground">To run the same test yourself, run this {{ runIn }}:</p>
            <div class="flex min-w-0 items-center gap-2">
              <code class="min-w-0 flex-1 truncate rounded-md border border-border px-2 py-1 font-mono text-sm well select-text">{{ test.command }}</code>
              <Tip content="Copy the command">
                <Button type="button" variant="outline" size="sm" @click="copyText(test.command, 'the command')"><CopyIcon />Copy</Button>
              </Tip>
            </div>
            <details class="mt-1">
              <summary class="cursor-default text-muted-foreground select-none">ssh's output</summary>
              <pre class="mt-1 max-h-60 overflow-auto rounded-md border border-border p-2 font-mono text-xs whitespace-pre-wrap well select-text scrollbar-thin">{{ test.output }}</pre>
            </details>
          </div>
        </section>

        <p v-if="setupError" class="text-destructive">{{ setupError }}</p>
        <p v-else-if="!setup" class="flex items-center gap-2 text-muted-foreground"><LoaderCircleIcon class="size-3.5 animate-spin" />Reading the SSH setup {{ where }}…</p>

        <template v-if="setup">
          <section class="grid gap-1.5">
            <h3 class="font-semibold">Git and ssh {{ where }}</h3>
            <dl class="grid grid-cols-[max-content_minmax(0,1fr)] gap-x-4 gap-y-1">
              <dt class="text-muted-foreground">Git</dt>
              <dd class="break-all select-text">{{ setup.gitExecutable }} <span class="text-muted-foreground">({{ setup.gitVersion }})</span></dd>
              <dt class="text-muted-foreground">ssh</dt>
              <dd class="break-all select-text">
                {{ setup.sshProgram || setup.sshCommand }} <span class="text-muted-foreground">({{ SOURCE_TEXT[setup.sshSource] ?? setup.sshSource }})</span>
              </dd>
              <template v-if="setup.sshSource !== 'default'">
                <dt class="text-muted-foreground">Command</dt>
                <dd class="font-mono text-sm break-all select-text">{{ setup.sshCommand }}</dd>
              </template>
              <dt class="text-muted-foreground">Version</dt>
              <dd class="break-all select-text">{{ setup.sshVersion }}</dd>
              <template v-if="setup.hostname">
                <dt class="text-muted-foreground">Logs in as</dt>
                <dd class="select-text">{{ setup.user }}@{{ setup.hostname }}, port {{ setup.port }}</dd>
                <dt class="text-muted-foreground">IdentitiesOnly</dt>
                <dd>{{ setup.identitiesOnly ? "Yes: only the key files below are offered" : "No: agent keys are offered as well" }}</dd>
              </template>
            </dl>
          </section>

          <section class="grid gap-1.5">
            <h3 class="font-semibold">Key files ssh tries, in order</h3>
            <p v-if="setup.identityFiles.length === 0" class="text-muted-foreground">ssh named no key files for this host.</p>
            <ul v-else class="grid gap-1">
              <li v-for="f in setup.identityFiles" :key="f.path" class="grid gap-0.5" data-identity-file>
                <div class="flex min-w-0 items-center gap-2">
                  <span :class="cn('shrink-0 rounded-sm px-1.5 text-xs font-semibold', stateClass(f))">{{ STATE_TEXT[f.state] }}</span>
                  <span :class="cn('min-w-0 truncate select-text', f.state === 'missing' && 'text-muted-foreground')" :title="f.path">{{ f.path }}</span>
                  <span v-if="triedFor(f.key?.fingerprint)?.accepted" class="shrink-0 rounded-sm px-1.5 text-xs font-semibold badge-added">Accepted</span>
                  <span v-else-if="triedFor(f.key?.fingerprint)?.offered" class="shrink-0 rounded-sm px-1.5 text-xs font-semibold badge-deleted">Offered, refused</span>
                  <span v-if="f.key && agentFingerprints.has(f.key.fingerprint)" class="shrink-0 rounded-sm px-1.5 text-xs font-semibold badge-neutral">In agent</span>
                </div>
                <span v-if="f.key" class="pl-1 font-mono text-xs break-all text-muted-foreground select-text">{{ keyText(f.key) }}</span>
              </li>
            </ul>
          </section>

          <section class="grid gap-1.5">
            <h3 class="font-semibold">SSH agent {{ where }}</h3>
            <p v-if="setup.agent.state === 'unavailable'" class="text-muted-foreground">No agent: ssh-add could not reach an SSH agent from where Git runs.</p>
            <p v-else-if="setup.agent.state === 'empty'" class="text-muted-foreground">The agent is running and holds no keys.</p>
            <p v-else-if="setup.agent.state === 'failed'" class="break-words text-destructive">{{ setup.agent.detail || "ssh-add failed." }}</p>
            <ul v-else class="grid gap-1">
              <li v-for="k in agentKeys" :key="k.fingerprint" class="flex min-w-0 items-center gap-2">
                <span class="min-w-0 font-mono text-xs break-all select-text">{{ keyText(k) }}</span>
                <span v-if="triedFor(k.fingerprint)?.accepted" class="shrink-0 rounded-sm px-1.5 text-xs font-semibold badge-added">Accepted</span>
                <span v-else-if="triedFor(k.fingerprint)?.offered" class="shrink-0 rounded-sm px-1.5 text-xs font-semibold badge-deleted">Offered, refused</span>
              </li>
            </ul>
          </section>
        </template>
      </div>

      <DialogFooter>
        <Button type="button" variant="outline" :disabled="testing" @click="runTest">
          <LoaderCircleIcon v-if="testing" class="animate-spin" />
          Test again
        </Button>
        <Button type="button" @click="dialogs.resolve('sshDiagnose', null)">Close</Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
</template>
