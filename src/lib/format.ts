// Pure display helpers — the unit-test surface for formatting.

/** Human-readable byte counts: 0 / 999 B / 1.0 KiB / 1.5 MiB / … */
export function humanizeBytes(bytes: number): string {
  if (bytes < 1024) return `${Math.max(0, Math.round(bytes))} B`;
  const units = ["KiB", "MiB", "GiB", "TiB"];
  let value = bytes;
  let unit = -1;
  do {
    value /= 1024;
    unit += 1;
  } while (value >= 1024 && unit < units.length - 1);
  // One decimal place while small, plain integers once it stops mattering.
  const text = value >= 10 ? value.toFixed(0) : value.toFixed(1);
  return `${text} ${units[unit]}`;
}

/** Uptime as mm:ss under an hour, h:mm:ss above. */
export function formatUptime(totalSecs: number): string {
  const secs = Math.max(0, Math.floor(totalSecs));
  const h = Math.floor(secs / 3600);
  const m = Math.floor((secs % 3600) / 60);
  const s = secs % 60;
  const mm = String(m).padStart(2, "0");
  const ss = String(s).padStart(2, "0");
  return h > 0 ? `${h}:${mm}:${ss}` : `${mm}:${ss}`;
}

/** Bytes/second for the throughput readout. */
export function formatRate(bytesPerSec: number): string {
  const rate = humanizeBytes(bytesPerSec);
  return rate === "0 B" ? "0 B/s" : `${rate}/s`;
}
