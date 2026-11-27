<script setup lang="ts">
import { computed, ref, shallowRef, watch } from "vue";
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
 *
 * It also lets another key be tried: each private key in ~/.ssh there can be offered to the
 * server by itself, and one that logs in can be set for this repository, which writes
 * core.sshCommand in the repository's own config.
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
  otherKeys.value = null;
  keysError.value = null;
  trying.value = null;
  tried.value = new Map();
  saving.value = false;
  saved.value = null;
  saveError.value = null;
  if (r) {
    void loadSetup();
    void runTest();
    void loadKeys();
  }
});

// ---- Trying another key ----------------------------------------------------------------------

/** The private key files in ~/.ssh where Git runs; null while they are being read. */
const otherKeys = ref<IdentityFile[] | null>(null);
const keysError = ref<string | null>(null);
/** The key being tried now, by path. One at a time: each may ask for its passphrase. */
const trying = ref<string | null>(null);
/** What trying each key gave, by path: the test, or why it could not run. Replaced, never mutated. */
const tried = shallowRef(new Map<string, SshTest | string>());
const saving = ref(false);
/** The key set for the repository in this dialog, and the command that was saved. */
const saved = ref<{ path: string; command: string } | null>(null);
const saveError = ref<string | null>(null);

async function loadKeys(): Promise<void> {
  const r = request.value;
  if (!r) return;
  try {
    const keys = await unwrap(commands.remoteSshKeys(r.key, r.dest, r.port));
    if (request.value === r) otherKeys.value = keys;
  } catch (e) {
    if (request.value === r) keysError.value = errorMessage(e);
  }
}

/** Logs in offering only this key. It may ask for the key's passphrase, as a fetch would. */
async function tryKey(path: string): Promise<void> {
  const r = request.value;
  if (!r || trying.value !== null || saving.value) return;
  trying.value = path;
  let result: SshTest | string;
  try {
    result = await unwrap(commands.remoteSshTestKey(r.key, r.dest, r.port, path));
  } catch (e) {
    result = errorMessage(e);
  }
  if (request.value === r) {
    tried.value = new Map(tried.value).set(path, result);
    trying.value = null;
  }
}

/** Sets the key for this repository, then reads the setup and runs the plain test again with it. */
async function useKey(path: string): Promise<void> {
  const r = request.value;
  if (!r || saving.value || trying.value !== null) return;
  saving.value = true;
  saveError.value = null;
  try {
    const command = await unwrap(commands.remoteSshUseKey(r.key, r.dest, r.port, path));
    if (request.value !== r) return;
    saved.value = { path, command };
    void loadSetup();
    void runTest();
  } catch (e) {
    if (request.value === r) saveError.value = errorMessage(e);
  } finally {
    saving.value = false;
  }
}

function triedTest(path: string): SshTest | null {
  const t = tried.value.get(path);
  return t !== undefined && typeof t !== "string" ? t : null;
}
function triedError(path: string): string | null {
  const t = tried.value.get(path);
  return typeof t === "string" ? t : null;
}

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
        `${locked.length === 1 ? `${locked[0].path} is` : `${locked.length} key files are`} encrypted in the old PEM format with no .pub file beside ${locked.length === 1 ? "it" : "them"}. ssh needs the passphrase to use such a key, and this test never asks for one, so it was skipped here. When you fetch, pull or push, StashPanda asks for the passphrase. Loading the key into the SSH agent, or converting it in Settings › SSH Keys, avoids being asked.`,
      );
    }
    if (s.agent.state === "unavailable") out.push(`No SSH agent is reachable ${where.value}, so ssh only has the key files below.`);
    if (s.identitiesOnly && agentKeys.value.length > 0) {
      out.push("IdentitiesOnly is on for this host, so ssh offers only the key files below, not other keys in the agent.");
    }
  }
  return out;
});

function outcomeLabel(outcome: SshTest["outcome"] | undefined): string {
  switch (outcome) {
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
}
const outcomeText = computed(() => outcomeLabel(test.value?.outcome));

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

        <!-- Each private key there, to offer to the server by itself and, if it logs in, to set for this repository. -->
        <section class="grid gap-1.5" data-try-keys>
          <h3 class="font-semibold">Try another key</h3>
          <p class="text-muted-foreground">
            These are the private keys in ~/.ssh {{ where }}. Try logs in to {{ request.dest }} offering only that key, and may ask for its
            passphrase. A key that logs in can be set for this repository, so that Git offers it and no other here.
          </p>
          <p v-if="keysError" class="break-words text-destructive">{{ keysError }}</p>
          <p v-else-if="otherKeys === null" class="flex items-center gap-2 text-muted-foreground">
            <LoaderCircleIcon class="size-3.5 animate-spin" />Reading the keys {{ where }}…
          </p>
          <p v-else-if="otherKeys.length === 0" class="text-muted-foreground">No private key files were found in ~/.ssh {{ where }}.</p>
          <ul v-else class="grid gap-1.5">
            <li v-for="k in otherKeys" :key="k.path" class="grid gap-0.5" data-try-key>
              <div class="flex min-w-0 items-center gap-2">
                <span v-if="k.state !== 'present'" :class="cn('shrink-0 rounded-sm px-1.5 text-xs font-semibold', stateClass(k))">{{ STATE_TEXT[k.state] }}</span>
                <span class="min-w-0 flex-1 truncate select-text" :title="k.path">{{ k.path }}</span>
                <span
                  v-if="triedTest(k.path)"
                  :class="cn('shrink-0 rounded-sm px-1.5 text-xs font-semibold', triedTest(k.path)?.outcome === 'authenticated' ? 'badge-added' : 'badge-deleted')"
                  data-try-outcome
                >
                  {{ outcomeLabel(triedTest(k.path)?.outcome) }}
                </span>
                <Button
                  v-if="triedTest(k.path)?.outcome === 'authenticated'"
                  type="button"
                  size="sm"
                  :disabled="saving || trying !== null || saved?.path === k.path"
                  data-use-key
                  @click="useKey(k.path)"
                >
                  <LoaderCircleIcon v-if="saving" class="animate-spin" />
                  {{ saved?.path === k.path ? "In Use for This Repository" : "Use for This Repository" }}
                </Button>
                <Button type="button" variant="outline" size="sm" :disabled="trying !== null || saving" data-try @click="tryKey(k.path)">
                  <LoaderCircleIcon v-if="trying === k.path" class="animate-spin" />
                  Try
                </Button>
              </div>
              <span v-if="k.key" class="pl-1 font-mono text-xs break-all text-muted-foreground select-text">{{ keyText(k.key) }}</span>
              <p v-if="triedError(k.path)" class="pl-1 break-words text-destructive">{{ triedError(k.path) }}</p>
              <details v-else-if="triedTest(k.path) && triedTest(k.path)?.outcome !== 'authenticated'" class="pl-1">
                <summary class="cursor-default text-muted-foreground select-none">ssh's output for this key</summary>
                <pre class="mt-1 max-h-48 overflow-auto rounded-md border border-border p-2 font-mono text-xs whitespace-pre-wrap well select-text scrollbar-thin">{{ triedTest(k.path)?.output }}</pre>
              </details>
            </li>
          </ul>
          <p v-if="saved" class="rounded-md border border-border px-2.5 py-1.5 well" data-key-saved>
            Git now offers {{ saved.path }}, and only that key, for this repository's SSH remotes. This is the setting
            <span class="font-mono text-sm">core.sshCommand</span> in the repository's own config:
            <span class="font-mono text-sm break-all select-text">{{ saved.command }}</span>. Remove it under Repository Settings to go back to ssh's own choice
            of keys.
          </p>
          <p v-if="saveError" class="break-words text-destructive">{{ saveError }}</p>
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
