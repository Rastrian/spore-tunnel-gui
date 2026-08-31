export interface AppConfig {
  bore_server_host: string;
  bore_server_port?: number;
  local_host: string;
  local_port: number;
  remote_port: number;
  profile_name?: string;
}

export interface TunnelStatus {
  state: "idle" | "starting" | "connected" | "failed" | "stopped";
  local_address: string;
  remote_address?: string;
  assigned_remote_port?: number;
  pid?: number;
  last_error?: string;
  logs: string[];
}
