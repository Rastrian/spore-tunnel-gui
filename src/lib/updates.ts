// Update-check UI state helpers: the "last checked" timestamp survives
// restarts in localStorage (injected so the node-env vitest run can use a
// fake instead of the real Storage), the relative-time formatter is pure.

const LAST_CHECKED_KEY = "sporeTunnel.lastUpdateCheck";

/** Minimal Storage surface the helpers need (tests fake it). */
export type StorageLike = Pick<Storage, "getItem" | "setItem">;

/** Relative "Checked N ago" label for the settings screen. Pure. */
export function formatLastChecked(checkedAt: number | null, now = Date.now()): string {
  if (checkedAt === null) return "Never checked";
  const secs = Math.max(0, Math.floor((now - checkedAt) / 1000));
  if (secs < 45) return "Checked just now";
  const mins = Math.floor(secs / 60);
  if (mins < 60) return `Checked ${mins} minute${mins === 1 ? "" : "s"} ago`;
  const hours = Math.floor(mins / 60);
  if (hours < 24) return `Checked ${hours} hour${hours === 1 ? "" : "s"} ago`;
  return `Checked on ${new Date(checkedAt).toLocaleDateString()}`;
}

/** Stored timestamp of the last check, or null when none/garbage. */
export function readLastChecked(storage?: StorageLike): number | null {
  try {
    const raw = storage?.getItem(LAST_CHECKED_KEY);
    if (raw === null || raw === undefined) return null;
    const n = Number(raw);
    return Number.isFinite(n) && n > 0 ? n : null;
  } catch {
    return null; // storage unavailable (private mode & friends)
  }
}

/** Persist the timestamp of a fresh check. Never throws. */
export function writeLastChecked(now = Date.now(), storage?: StorageLike): void {
  try {
    storage?.setItem(LAST_CHECKED_KEY, String(now));
  } catch {
    // Unavailable storage only costs the "last checked" nicety.
  }
}

/** The real browser storage (undefined under vitest's node environment). */
export function defaultStorage(): StorageLike | undefined {
  return globalThis.localStorage;
}
