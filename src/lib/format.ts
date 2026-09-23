import type { Connection, HostInfo, ResolvedTransport } from "@/lib/bindings";

/** Short badge text such as "Local", "WSL Ubuntu" or "SSH host". */
export function transportLabel(transport: ResolvedTransport | null | undefined): string {
  if (!transport) return "Unresolved";
  switch (transport.type) {
    case "local":
      return "Local";
    case "wsl":
      return `WSL ${transport.distro}`;
    case "ssh":
      return `SSH ${transport.host}`;
  }
}

/** Longer status-bar text for a transport, including the executable or SSH user. */
export function transportDetail(transport: ResolvedTransport | null | undefined): string {
  if (!transport) return "Unresolved";
  switch (transport.type) {
    case "local":
      return `Local (${transport.gitExecutable})`;
    case "wsl":
      return `WSL (${transport.distro})`;
    case "ssh": {
      const user = transport.user ? `${transport.user}@` : "";
      const port = transport.port !== null ? `:${transport.port}` : "";
      return `SSH (${user}${transport.host}${port})`;
    }
  }
}

export function connectionLabel(connection: Connection | undefined): string {
  if (!connection) return "Auto";
  switch (connection.type) {
    case "auto":
      return "Auto";
    case "local":
      return "Local";
    case "wsl":
      return `WSL ${connection.distro}`;
    case "ssh":
      return `SSH ${connection.host}`;
  }
}

export function hostLabel(host: HostInfo | null | undefined): string {
  if (!host) return "";
  switch (host.type) {
    case "windows":
      return "Windows";
    case "linuxWsl":
      return `Linux (WSL: ${host.distro})`;
    case "linux":
      return "Linux";
    case "macOs":
      return "macOS";
  }
}

/** Last path segment of a Windows, UNC, Linux or `user@host:/path` string. */
export function pathBasename(path: string): string {
  const trimmed = path.replace(/[\\/]+$/, "");
  const idx = Math.max(trimmed.lastIndexOf("/"), trimmed.lastIndexOf("\\"));
  return idx >= 0 ? trimmed.slice(idx + 1) : trimmed;
}

export function formatDurationMs(ms: number | null): string {
  if (ms === null) return "";
  if (ms < 1000) return `${Math.round(ms)} ms`;
  return `${(ms / 1000).toFixed(2)} s`;
}

export function formatClockTime(epochMs: number): string {
  const d = new Date(epochMs);
  const hh = String(d.getHours()).padStart(2, "0");
  const mm = String(d.getMinutes()).padStart(2, "0");
  const ss = String(d.getSeconds()).padStart(2, "0");
  return `${hh}:${mm}:${ss}`;
}

/** First 7 hex characters of a commit oid (or the whole string when shorter). */
export function shortOid(oid: string): string {
  return oid.length > 7 ? oid.slice(0, 7) : oid;
}

/** "3 minutes ago", "2 days ago", "in 5 hours" for a Unix-seconds timestamp. */
export function formatRelativeTime(epochSec: number, nowMs: number = Date.now()): string {
  const diffSec = Math.round(nowMs / 1000 - epochSec);
  const abs = Math.abs(diffSec);
  const future = diffSec < 0;
  const units: [number, string][] = [
    [60, "second"],
    [60, "minute"],
    [24, "hour"],
    [7, "day"],
    [4.348, "week"],
    [12, "month"],
    [Number.POSITIVE_INFINITY, "year"],
  ];
  if (abs < 45) return future ? "in a moment" : "just now";
  let value = abs;
  let label = "second";
  for (const [size, name] of units) {
    label = name;
    if (value < size) break;
    value /= size;
  }
  const n = Math.max(1, Math.round(value));
  const text = `${n} ${label}${n === 1 ? "" : "s"}`;
  return future ? `in ${text}` : `${text} ago`;
}

/** "2026-09-18 14:05" in local time for a Unix-seconds timestamp. */
export function formatAbsoluteTime(epochSec: number): string {
  const d = new Date(epochSec * 1000);
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}`;
}

/** Full ISO 8601 string for tooltips. */
export function isoTime(epochSec: number): string {
  return new Date(epochSec * 1000).toISOString();
}

/** Up to two initials from a person's name ("Michael Smith" -> "MS", "michael" -> "M"). */
export function initials(name: string): string {
  const parts = name.trim().split(/\s+/).filter((p) => p.length > 0);
  if (parts.length === 0) return "?";
  const first = parts[0] as string;
  if (parts.length === 1) return first.slice(0, 1).toUpperCase();
  const last = parts[parts.length - 1] as string;
  return (first.slice(0, 1) + last.slice(0, 1)).toUpperCase();
}

/** Small non-negative 32-bit string hash (FNV-1a), stable across sessions. */
export function hashString(s: string): number {
  let h = 0x811c9dc5;
  for (let i = 0; i < s.length; i += 1) {
    h ^= s.charCodeAt(i);
    h = Math.imul(h, 0x01000193);
  }
  return h >>> 0;
}
