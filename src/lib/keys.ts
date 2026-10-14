/**
 * What the SSH Keys page in Settings and the Add Repository dialog's key picker share: how a key
 * is labelled, the folders scanned, the system keychain's status, and asking for a passphrase.
 */
import { commands, type Connection, type KeychainStatus, type KeyFormat, type KeyInfo, type RepoEntry } from "@/lib/bindings";
import { IpcError, unwrap } from "@/lib/ipc";
import { useDialogsStore } from "@/stores/dialogs";
import { useSettingsStore } from "@/stores/settings";

/** The format badge: PPK 2, PPK 3, PEM, OpenSSH or PKCS#8. */
export function keyFormatLabel(format: KeyFormat): string {
  switch (format) {
    case "ppkV2":
      return "PPK 2";
    case "ppkV3":
      return "PPK 3";
    case "openSsh":
      return "OpenSSH";
    case "pemRsa":
    case "pemEc":
    case "pemDsa":
      return "PEM";
    case "pkcs8":
    case "pkcs8Encrypted":
      return "PKCS#8";
    case "publicOpenSsh":
    case "publicRfc4716":
      return "Public key";
    case "unknown":
      return "Unknown";
  }
}

/** `ssh-ed25519` as ED25519, `ecdsa-sha2-nistp256` as ECDSA, `ssh-rsa` as RSA, `ssh-dss` as DSA. */
export function keyAlgorithmName(algorithm: string | null): string | null {
  if (!algorithm) return null;
  const a = algorithm.toLowerCase();
  if (a.includes("ed25519")) return "ED25519";
  if (a.includes("ecdsa")) return "ECDSA";
  if (a.includes("rsa")) return "RSA";
  if (a.includes("dss") || a.includes("dsa")) return "DSA";
  return algorithm;
}

/** `RSA 4096`, `ED25519` (its size never varies, so it is left out), or null when unknown. */
export function keyAlgorithmLabel(key: KeyInfo): string | null {
  const name = keyAlgorithmName(key.algorithm);
  if (!name) return null;
  return key.bits !== null && name !== "ED25519" ? `${name} ${key.bits}` : name;
}

/** DSA keys are read and converted, but OpenSSH 9 servers refuse them. */
export function isDeprecatedKey(key: KeyInfo): boolean {
  return keyAlgorithmName(key.algorithm) === "DSA";
}

/** The folders the user added on the SSH Keys page, scanned besides the default ones. */
export function extraKeyDirs(): string[] {
  return useSettingsStore().settings?.keys?.extraDirs ?? [];
}

/** Every key in the default folders and the added ones. */
export async function scanKeys(): Promise<KeyInfo[]> {
  return unwrap(commands.keysScan(extraKeyDirs()));
}

let keychain: Promise<KeychainStatus> | null = null;

/** Whether passphrases can be stored in the system keychain. Asked once; the answer does not change while StashPanda runs. */
export function keychainStatus(): Promise<KeychainStatus> {
  keychain ??= unwrap(commands.keysKeychainStatus()).catch((e: unknown) => {
    keychain = null;
    throw e;
  });
  return keychain;
}

/**
 * Runs a key command, first with no passphrase (the backend uses a remembered one, and an
 * unencrypted key needs none), and again with the passphrase from the dialog when the backend
 * says the key needs one or the remembered one is wrong. `purpose` finishes "StashPanda needs
 * the passphrase …". Resolves undefined when the user cancelled the dialog.
 */
export async function withPassphrase<T>(key: KeyInfo, purpose: string, run: (passphrase: string | null) => Promise<T>): Promise<T | undefined> {
  let passphrase: string | null = null;
  for (;;) {
    try {
      return await run(passphrase);
    } catch (e) {
      const kind = e instanceof IpcError ? e.error.kind : null;
      if (kind !== "sshNeedsPassphrase" && kind !== "sshWrongPassphrase") throw e;
      const result = await useDialogsStore().open("passphrase", {
        path: key.path,
        keyName: key.fileName,
        purpose,
        wrong: kind === "sshWrongPassphrase",
      });
      if (!result) return undefined;
      passphrase = result.passphrase;
    }
  }
}

/** True when the entry is reached over SSH, set explicitly or resolved from a `user@host:/path` path. */
export function isSshEntry(entry: RepoEntry): boolean {
  const type = entry.connection?.type ?? "auto";
  return type === "ssh" || (type === "auto" && entry.resolvedTransport?.type === "ssh");
}

/**
 * The connection that makes `entry` log in with the key at `path`. An entry set to Auto becomes
 * an SSH connection with no host of its own, so the host still comes from its `user@host:/path`.
 */
export function connectionWithKey(entry: RepoEntry, path: string): Connection {
  const c = entry.connection;
  if (c?.type === "ssh") return { ...c, identityFile: path };
  return { type: "ssh", host: "", user: null, port: null, identityFile: path, remoteGit: null };
}
