// camelCase mirrors of the Rust types in src-tauri/src (config.rs,
// tunnel/supervisor.rs, tunnel/events.rs, commands.rs). Keep in sync.

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

export interface AppConfig {
  profiles: Profile[];
  activeProfileId?: string;
  ui: UiPrefs;
}

export type TunnelState =
  | "idle"
  | "starting"
  | "connected"
  | "failed"
  | "stopped";

export interface TunnelStatus {
  state: TunnelState;
  /** "Bore" | "Spore" once the handshake identified the server. */
  serverKind?: string;
  localAddress: string;
  remoteAddress?: string;
  assignedRemotePort?: number;
  uptimeSecs?: number;
  bytesUp?: number;
  bytesDown?: number;
  reconnects?: number;
  lastError?: string;
  logs: string[];
}

export interface LogEntry {
  index: number;
  ts: number;
  level: "info" | "error";
  line: string;
}

export interface ProfileStatus {
  profileId: string;
  status: TunnelStatus;
}

export interface DetectedService {
  port: number;
  name: string;
}
