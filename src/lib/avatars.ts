/**
 * Author pictures from Gravatar, used when `appearance.avatarSource` is "gravatar". Gravatar finds
 * a picture by the SHA-256 hash of the email address, so only that hash leaves this computer. The
 * hash is worked out once per address; `d=404` makes an address without a picture fail to load, and
 * the row then keeps its initials.
 */
import { shallowReactive } from "vue";

/** By lower-case address: the picture's URL, or null when Gravatar has none for it. */
const urls = shallowReactive(new Map<string, string | null>());
const hashing = new Set<string>();

function keyOf(email: string): string {
  return email.trim().toLowerCase();
}

async function hash(key: string): Promise<void> {
  const digest = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(key));
  const hex = [...new Uint8Array(digest)].map((b) => b.toString(16).padStart(2, "0")).join("");
  // 32 pixels for a 16-pixel circle, so it stays sharp on screens scaled 200%.
  urls.set(key, `https://gravatar.com/avatar/${hex}?s=32&d=404`);
}

/** The picture's URL for `email`, or null while its hash is being worked out or when there is none. */
export function gravatarUrl(email: string): string | null {
  const key = keyOf(email);
  if (key.length === 0) return null;
  if (urls.has(key)) return urls.get(key) ?? null;
  if (!hashing.has(key)) {
    hashing.add(key);
    // Without crypto.subtle (not a secure context) the initials stay.
    void hash(key).catch(() => urls.set(key, null));
  }
  return null;
}

/** Called when the picture failed to load: Gravatar has none for this address. */
export function gravatarMissing(email: string): void {
  urls.set(keyOf(email), null);
}
