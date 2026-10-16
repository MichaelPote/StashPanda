import type { GitError } from "@/lib/bindings";

/** The shape every generated command in `bindings.ts` resolves to. */
export type IpcResult<T> = { status: "ok"; data: T } | { status: "error"; error: GitError };

/** Thrown by `unwrap()` so callers can `catch` and still reach the typed `GitError`. */
export class IpcError extends Error {
  readonly error: GitError;

  constructor(error: GitError) {
    super(GitErrorMessage(error));
    this.name = "IpcError";
    this.error = error;
  }
}

/** Turns a `Result<T, GitError>` object from a generated command into `T`, or throws an `IpcError`. */
export async function unwrap<T>(result: Promise<IpcResult<T>> | IpcResult<T>): Promise<T> {
  const r = await result;
  if (r.status === "ok") return r.data;
  throw new IpcError(r.error);
}

/** Plain-English message for anything a command call can throw. */
export function errorMessage(e: unknown): string {
  if (e instanceof IpcError) return e.message;
  if (e instanceof Error) return e.message;
  return String(e);
}

function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KiB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MiB`;
}

function assertNever(value: never): never {
  throw new Error(`Unhandled GitError variant: ${JSON.stringify(value)}`);
}

/** One plain-English sentence for every `GitError` variant. */
export function GitErrorMessage(error: GitError): string {
  switch (error.kind) {
    case "gitNotFound":
      return error.searched.length > 0
        ? `Git was not found. Searched: ${error.searched.join(", ")}.`
        : "Git was not found on this machine.";
    case "gitVersionTooOld":
      return `Git ${error.found} is too old. StashPanda needs Git ${error.required} or newer.`;
    case "wslNotAvailable":
      return "WSL is not available on this machine.";
    case "wslDistroNotFound":
      return error.available.length > 0
        ? `The WSL distribution "${error.distro}" was not found. Available distributions: ${error.available.join(", ")}.`
        : `The WSL distribution "${error.distro}" was not found, and no WSL distributions are installed.`;
    case "sshConnectFailed":
      return `Could not connect to ${error.host} over SSH: ${error.detail}`;
    case "sshAuthFailed":
      return error.methodsTried.length > 0
        ? `SSH authentication to ${error.host} failed. Methods tried: ${error.methodsTried.join(", ")}.`
        : `SSH authentication to ${error.host} failed.`;
    case "sshHostKeyUnknown":
      return `The SSH host key for ${error.host} is not known yet (fingerprint ${error.fingerprint}).`;
    case "sshHostKeyChanged":
      return `The SSH host key for ${error.host} has changed (fingerprint ${error.fingerprint}). Check the host before continuing.`;
    case "sshKeyUnsupported":
      return `The SSH key ${error.path} is in an unsupported format (${error.detectedHeader}).`;
    case "sshNeedsPassphrase":
      return `The SSH key ${error.path} needs a passphrase.`;
    case "sshWrongPassphrase":
      return `The passphrase for the SSH key ${error.path} is wrong.`;
    case "keyParseError":
      return `Could not read the key ${error.path} as ${error.format}: ${error.detail}`;
    case "sshUnsupportedFeature":
      return `The SSH feature "${error.feature}" is not supported.`;
    case "transportDisconnected":
      return `The connection to the repository was lost: ${error.detail}`;
    case "notARepository":
      return `${error.path} is not a Git repository.`;
    case "repoPathNotFound":
      return `The path ${error.path} does not exist.`;
    case "unsafeRepository":
      return `Git refuses to use ${error.path} because it is owned by another user (see git config safe.directory).`;
    case "indexLocked":
      return `The Git index is locked by ${error.lockPath}. Another Git process may be running.`;
    case "configLocked":
      return `Git could not save to the repository's settings file, because ${error.lockPath} is there. Another Git process may be saving to it.`;
    case "invalidPath":
      return `The path ${error.path} is not valid: ${error.reason}`;
    case "objectNotFound":
      return `The Git object ${error.spec} was not found.`;
    case "tooLarge":
      return `The content is too large to show (${formatBytes(error.bytes)}; the limit is ${formatBytes(error.limit)}).`;
    case "commandFailed": {
      const code = error.exitCode === null ? "" : ` with exit code ${error.exitCode}`;
      const stderr = error.stderr.trim();
      return stderr.length > 0 ? `${error.label} failed${code}: ${stderr}` : `${error.label} failed${code}.`;
    }
    case "parseError":
      return `Could not parse ${error.what}: ${error.detail}`;
    case "timeout":
      return `The Git command timed out after ${error.secs} seconds.`;
    case "cancelled":
      return "The operation was cancelled.";
    case "repoNotOpen":
      return `The repository ${error.repoId} is not open.`;
    case "staleVersion":
      return `The requested data (version ${error.requested}) is out of date; the current version is ${error.current}.`;
    case "io":
      return `A file system error occurred: ${error.detail}`;
    case "changedOnDisk":
      return `${error.path} changed on disk since it was read, so it was not saved over.`;
    case "internal":
      return `Internal error: ${error.detail}`;
    default:
      return assertNever(error);
  }
}
