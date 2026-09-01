// camelCase mirrors of the Rust types in src-tauri/src (config.rs,
// tunnel/supervisor.rs, tunnel/events.rs, commands.rs). Keep in sync with
// docs/EVENTS.md — the frozen event/command contract.

export type Theme = "dark" | "light" | "system";

export interface Profile {
  id: string;
  name: string;
  serverHost: string;
  serverPort: number;
  localHost: string;
  localPort: number;
  /** 0 = random (server assigns). */
  remotePort: number;
  autostart: boolean;
  autoReconnect: boolean;
}

export interface UiPrefs {
  theme: Theme;
  startMinimized: boolean;
  closeToTray: boolean;
}

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
  /** Dialect the handshake identified; null until the first connect. */
  serverKind: ServerKind | null;
  /** The local service being exposed, `host:port`. */
  localAddress: string;
  /** Public `host:port`; null while not connected. May change after a reconnect. */
  remoteAddress: string | null;
  assignedRemotePort: number | null;
  uptimeSecs: number;
  bytesUp: number;
  bytesDown: number;
  reconnects: number;
  lastError: string | null;
  /**
   * Convenience snapshot only — the live stream is the `tunnel://log`
   * event. Do not render from this array.
   */
  logs: string[];
}

export interface StatusEvent {
  profileId: string;
  /** Full snapshot — replace any previous copy wholesale. */
  status: TunnelStatus;
}

export interface LogEvent {
  profileId: string;
  /** Strictly increasing within a run; resets to 0 on `start_tunnel`. */
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

/** One entry of `get_all_status` (an array, not a record). */
export interface ProfileStatus {
  profileId: string;
  status: TunnelStatus;
}

export interface DetectedService {
  port: number;
  name: string;
}

/** Result of `check_for_updates` (src-tauri/src/updates.rs). */
export interface UpdateStatus {
  current: string;
  latest: string;
  updateAvailable: boolean;
  /** Release page to open in a browser. */
  url: string;
}
