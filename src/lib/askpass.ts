import type { UnlistenFn } from "@tauri-apps/api/event";
import { commands, events, type AskpassRequest } from "@/lib/bindings";
import { keychainStatus } from "@/lib/keys";
import { useDialogsStore, type DialogKind } from "@/stores/dialogs";

/**
 * Answers Git and the `ssh` it starts when they ask for something through the askpass helper
 * (`src-tauri/src/askpass.rs`). The backend answers by itself with a passphrase it remembers;
 * what arrives here needs the user. One dialog at a time: a second request waits for the first.
 */

/** The dialog each request opens, and how that dialog closes as cancelled. */
function dialogFor(request: AskpassRequest): DialogKind {
  switch (request.kind.type) {
    case "passphrase":
      return "passphrase";
    case "confirm":
      return "confirm";
    case "secret":
    case "text":
      return "askpass";
  }
}

/**
 * ssh's question about a server it has not seen, without its last line, `Are you sure you want
 * to continue connecting (yes/no/[fingerprint])?`, which the dialog's buttons stand in for.
 */
export function hostKeyQuestion(prompt: string): string {
  const lines = prompt.split("\n").filter((line) => !/\(yes\/no/.test(line));
  return lines.join("\n").trim();
}

/** Opens the dialog for `request` and resolves with the answer for the helper: null when cancelled. */
async function ask(request: AskpassRequest): Promise<string | null> {
  const dialogs = useDialogsStore();
  const kind = request.kind;
  switch (kind.type) {
    case "passphrase": {
      const keychain = await keychainStatus().catch(() => null);
      const result = await dialogs.open("passphrase", {
        path: kind.path,
        keyName: kind.keyName,
        purpose: `so that git ${request.command} can log in to the server`,
        wrong: kind.wrong,
        // Git asks again for every command, so the choice that stops that for good is the one
        // selected first: the system keychain where there is one, else until StashPanda closes.
        remember: keychain?.available ? "keychain" : "session",
      });
      return result?.passphrase ?? null;
    }
    case "confirm": {
      const question = hostKeyQuestion(kind.prompt);
      const yes = await dialogs.open("confirm", {
        title: "Connect to this server?",
        message: `git ${request.command} is connecting to a server that ssh has not seen before.\n\n${question}\n\nConnect records the server's key in known_hosts, so this is asked once. Check the fingerprint against the one the server's owner publishes if you can.`,
        confirmLabel: "Connect",
      });
      return yes ? "yes" : null;
    }
    case "secret":
    case "text":
      return dialogs.open("askpass", { prompt: kind.prompt, secret: kind.type === "secret", command: request.command });
  }
}

/** The request a dialog is open for. */
let current: { id: string; dialog: DialogKind } | null = null;
const waiting: AskpassRequest[] = [];

async function next(): Promise<void> {
  if (current !== null) return;
  const request = waiting.shift();
  if (!request) return;
  current = { id: request.id, dialog: dialogFor(request) };
  let answer: string | null = null;
  try {
    answer = await ask(request);
  } catch {
    answer = null;
  }
  current = null;
  // The helper waits for an answer however the dialog ended.
  await commands.askpassAnswer(request.id, answer).catch(() => undefined);
  void next();
}

/** The Git command a request belongs to ended or was stopped: its dialog closes, as cancelled. */
function close(id: string): void {
  const queued = waiting.findIndex((r) => r.id === id);
  if (queued >= 0) {
    waiting.splice(queued, 1);
    return;
  }
  if (current?.id !== id) return;
  const dialogs = useDialogsStore();
  if (current.dialog === "confirm") dialogs.resolve("confirm", false);
  else if (current.dialog === "passphrase") dialogs.resolve("passphrase", null);
  else dialogs.resolve("askpass", null);
}

/** Starts listening. Called once, when the app starts. */
export async function installAskpass(): Promise<UnlistenFn> {
  const unlistenRequest = await events.askpassRequest.listen((event) => {
    waiting.push(event.payload);
    void next();
  });
  const unlistenClosed = await events.askpassClosed.listen((event) => close(event.payload.id));
  return () => {
    unlistenRequest();
    unlistenClosed();
  };
}
