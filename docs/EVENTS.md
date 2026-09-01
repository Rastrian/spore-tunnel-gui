# Tunnel event contract (Phase 2 freeze)

This is the contract between the Rust backend and the Phase 3 frontend.
Source of truth: `src-tauri/src/tunnel/events.rs` (payload types) and
`src-tauri/src/tunnel/supervisor.rs` (`TunnelStatus`, log ring). If this
document and the Rust types disagree, the Rust types win — then fix this
document.

The backend pushes every status, log, and stats update to the webview as a
Tauri event. The frontend subscribes with `listen()` from
`@tauri-apps/api/event` and **never polls** for status, stats, or logs.
`invoke` is only for commands that *change* something (start/stop, profile
CRUD) and for one-shot hydration at startup (`get_all_status`,
`get_tunnel_log`).

All payloads are JSON with camelCase keys, exactly as serialized by the
Rust side. Event names are the string constants
`STATUS_EVENT` / `LOG_EVENT` / `STATS_EVENT` in `events.rs`.

Startup order matters: subscribe to the three events **first**, then
hydrate with `get_all_status()` (+ log backfill). That way nothing emitted
between hydration and subscription is lost.

---

## `tunnel://status`

Emitted whenever anything visible about a tunnel changes (state, assigned
port, error, counters). Payload:

| Field      | Type          | Description                                                                                          |
|------------|---------------|------------------------------------------------------------------------------------------------------|
| `profileId`| `string`      | UUID of the profile this tunnel belongs to.                                                          |
| `status`   | `TunnelStatus`| Full snapshot, see below. Always complete — the frontend can replace its previous copy wholesale.    |

### `TunnelStatus`

| Field               | Type                        | Description                                                                                                  |
|---------------------|-----------------------------|--------------------------------------------------------------------------------------------------------------|
| `state`             | `"idle" \| "starting" \| "connected" \| "failed" \| "stopped"` | Lifecycle state. A tunnel in automatic reconnect reports `"starting"` (there is no separate `reconnecting`). |
| `serverKind`        | `"Bore" \| "Spore" \| null` | Dialect the handshake identified. `null` until the first successful connect (i.e. during `"starting"`).     |
| `localAddress`      | `string`                    | The local service being exposed, `host:port`.                                                               |
| `remoteAddress`     | `string \| null`            | Public `host:port` visitors connect to. `null` while not connected. May change after a reconnect.           |
| `assignedRemotePort`| `number \| null`            | Numeric port the server assigned (last segment of `remoteAddress`; convenience for the copy-button chip).   |
| `uptimeSecs`        | `number`                    | Seconds since the current session connected; `0` while not connected.                                       |
| `bytesUp`           | `number`                    | Bytes pushed from the local service to visitors, since the tunnel started.                                  |
| `bytesDown`         | `number`                    | Bytes received from visitors, since the tunnel started.                                                     |
| `reconnects`        | `number`                    | How many times the tunnel died and was re-established in this run.                                          |
| `lastError`         | `string \| null`            | Human-readable reason for the latest failure/connect error; `null` when healthy.                            |
| `logs`              | `string[]`                  | Convenience snapshot of the last lines (same ring as `tunnel://log`, strings only). **The live stream is `tunnel://log`** — do not diff this array. |

### Emission rules

- Every state change emits; counter/uptime ticks ride along.
- Coalesced to **at most one event per second per tunnel**. Coalescing is
  trailing-edge: the last state within a burst is always delivered, so the
  frontend never ends up stuck on a stale intermediate state.
- Stop emits a final `"stopped"` event immediately (not coalesced).
- Reconnects emit `connected` again, possibly with a new `remoteAddress` /
  `assignedRemotePort` — always re-read the address from the event.

---

## `tunnel://log`

One event per log line — never coalesced, never dropped.

| Field       | Type                       | Description                                                                       |
|-------------|----------------------------|-----------------------------------------------------------------------------------|
| `profileId` | `string`                   | Owning profile.                                                                   |
| `index`     | `number`                   | Per-tunnel monotonic sequence number. See reset semantics below.                  |
| `line`      | `string`                   | The log line, already formatted for display (no timestamp/level prefix inside).   |
| `level`     | `"info" \| "error"`        | Severity.                                                                         |
| `ts`        | `number`                   | Unix epoch **milliseconds**.                                                      |

### `index` semantics

- Strictly increasing within a run; starts at `0`.
- Resets to `0` whenever the tunnel is (re)started via `start_tunnel` — on
  seeing a smaller `index` than your last seen one, drop your buffered log
  for that tunnel (the old run is gone).
- Keeps increasing across automatic reconnects within one run.
- Used for backfill resumption, see `get_tunnel_log` below.

---

## `tunnel://stats`

Throughput tick, emitted **once per second while the tunnel is running**
(state `starting` or `connected`). Not emitted while idle/failed/stopped.

| Field       | Type     | Description                                      |
|-------------|----------|--------------------------------------------------|
| `profileId` | `string` | Owning profile.                                  |
| `bytesUp`   | `number` | Cumulative bytes up (same value as in status).   |
| `bytesDown` | `number` | Cumulative bytes down.                           |
| `uptimeSecs`| `number` | Session uptime in seconds.                       |

`bytesUp`/`bytesDown` are cumulative counters since the tunnel started —
compute per-second deltas in the frontend for the throughput sparkline.

---

## TypeScript types

Paste-ready mirror of the payloads above (`src/lib/types.ts` in Phase 3;
keep in sync with this file).

```ts
export type TunnelState =
  | "idle"
  | "starting"
  | "connected"
  | "failed"
  | "stopped";

export type ServerKind = "Bore" | "Spore";
export type LogLevel = "info" | "error";

export interface TunnelStatus {
  state: TunnelState;
  serverKind: ServerKind | null;
  localAddress: string;
  remoteAddress: string | null;
  assignedRemotePort: number | null;
  uptimeSecs: number;
  bytesUp: number;
  bytesDown: number;
  reconnects: number;
  lastError: string | null;
  /** Convenience snapshot only — the live stream is `tunnel://log`. */
  logs: string[];
}

export interface StatusEvent {
  profileId: string;
  status: TunnelStatus;
}

export interface LogEvent {
  profileId: string;
  index: number;
  line: string;
  level: LogLevel;
  /** Unix epoch milliseconds. */
  ts: number;
}

export interface StatsEvent {
  profileId: string;
  bytesUp: number;
  bytesDown: number;
  uptimeSecs: number;
}

/** Backfill entry shape returned by `get_tunnel_log`. */
export interface LogEntry {
  index: number;
  ts: number;
  level: LogLevel;
  line: string;
}

export interface DetectedService {
  port: number;
  name: string;
}

export interface Profile {
  id: string;          // uuid
  name: string;
  serverHost: string;
  serverPort: number;  // default 7835
  localHost: string;   // default "127.0.0.1"
  localPort: number;   // default 25565
  remotePort: number;  // 0 = random assignment
  autostart: boolean;
  autoReconnect: boolean;
}
```

---

## Log backfill — `get_tunnel_log`

The backend keeps an in-memory ring of the **last 1024 lines per tunnel**
(same ring `tunnel://log` streams from). On mount, after subscribing to the
events, call:

```
get_tunnel_log(profileId, sinceIndex?)
```

| `sinceIndex`                  | Returns                                                        |
|-------------------------------|----------------------------------------------------------------|
| omitted / `null`              | Everything currently in the ring (full backfill), oldest first.|
| `Some(n)` / `n`               | Entries with `index` **strictly greater than** `n`, oldest first.|

Notes:

- Entries evicted from the 1024-line ring are skipped silently — after a
  long gap there may be a hole between your last seen `index` and the first
  entry returned. Entries are never re-numbered within a run.
- The ring (and thus `index`) resets on `start_tunnel` — see above.
- Backfill, then apply events with `index > lastSeen` — the same strictly-
  greater rule makes the live stream and the backfill compose without gaps
  or duplicates.

---

## Command reference (Phase 2 contract)

These land with the manager integration. All commands return
`Result<T, String>` — a rejected `invoke` carries the error string.
Profile fields: see `Profile` above.

| Command                 | Args                                   | Returns                        |
|-------------------------|----------------------------------------|--------------------------------|
| `list_profiles`         | —                                      | `Profile[]`                    |
| `save_profile`          | `profile: Profile`                     | `Profile` (id filled on create)|
| `set_active_profile`    | `profileId: string \| null`            | `void`                         |
| `set_profile_secret`    | `profileId: string`, `secret: string`  | `void` (OS keyring, `profile:<id>`) |
| `delete_profile`        | `profileId: string`                    | `void` (drops its keyring secret too) |
| `import_legacy`         | —                                      | `Profile \| null` (`null` = nothing to import) |
| `has_legacy_config`     | —                                      | `boolean`                      |
| `start_tunnel`          | `profileId: string`, `secret?: string` | `TunnelStatus` (initial, usually `"starting"`) |
| `stop_tunnel`           | `profileId?: string`                   | `void` (omitted = stop all)    |
| `get_status`            | `profileId?: string`                   | `TunnelStatus` (active profile when omitted; idle-shaped before first start) |
| `get_all_status`        | —                                      | `Record<profileId, TunnelStatus>` |
| `get_tunnel_log`        | `profileId: string`, `sinceIndex?: number \| null` | `LogEntry[]`        |
| `copy_address`          | `profileId?: string`                   | `string` (public `host:port`; rejects when not connected) |
| `open_config_folder`    | —                                      | `void`                         |
| `detect_local_service`  | —                                      | `DetectedService[]` (see `src-tauri/src/discover.rs`) |

Secrets never appear in profile objects or config files — they are written
through `set_profile_secret` (or the `start_tunnel` argument) into the OS
keyring only.

---

## Wiring example

Minimal zustand store fed exclusively by the three events plus the one-shot
hydration calls (Phase 3 expands this, the shape stays).

```ts
import { create } from "zustand";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";
import type { LogEntry, LogEvent, StatsEvent, StatusEvent, TunnelStatus } from "./types";

interface TunnelStore {
  statuses: Record<string, TunnelStatus>;
  logs: Record<string, LogEntry[]>;
}

export const useTunnels = create<TunnelStore>(() => ({
  statuses: {},
  logs: {},
}));

const MAX_CLIENT_LINES = 1024;

export async function initTunnelEvents(): Promise<UnlistenFn[]> {
  const unlistenStatus = await listen<StatusEvent>("tunnel://status", ({ payload }) => {
    const { profileId, status } = payload;
    useTunnels.setState((s) => ({
      statuses: { ...s.statuses, [profileId]: status },
    }));
  });

  const unlistenLog = await listen<LogEvent>("tunnel://log", ({ payload }) => {
    const { profileId, ...entry } = payload; // entry: { index, line, level, ts }
    useTunnels.setState((s) => {
      const buffered = s.logs[profileId] ?? [];
      const last = buffered.at(-1)?.index ?? -1;
      if (entry.index <= last) return s; // duplicate or pre-reset race
      return {
        logs: {
          ...s.logs,
          [profileId]: [...buffered, entry].slice(-MAX_CLIENT_LINES),
        },
      };
    });
  });

  const unlistenStats = await listen<StatsEvent>("tunnel://stats", ({ payload }) => {
    const { profileId, bytesUp, bytesDown, uptimeSecs } = payload;
    useTunnels.setState((s) => {
      const status = s.statuses[profileId];
      if (!status) return s; // no status yet — hydration will supply it
      return {
        statuses: {
          ...s.statuses,
          [profileId]: { ...status, bytesUp, bytesDown, uptimeSecs },
        },
      };
    });
  });

  // One-shot hydration AFTER subscribing, so no event is missed.
  const all = await invoke<Record<string, TunnelStatus>>("get_all_status");
  for (const [profileId, status] of Object.entries(all)) {
    const buffered = useTunnels.getState().logs[profileId] ?? [];
    const last = buffered.at(-1)?.index ?? null;
    const entries = await invoke<LogEntry[]>("get_tunnel_log", {
      profileId,
      sinceIndex: last,
    });
    useTunnels.setState((s) => ({
      statuses: { ...s.statuses, [profileId]: status },
      logs: {
        ...s.logs,
        [profileId]: last === null ? entries : [...buffered, ...entries].slice(-MAX_CLIENT_LINES),
      },
    }));
  }

  return [unlistenStatus, unlistenLog, unlistenStats];
}
```
