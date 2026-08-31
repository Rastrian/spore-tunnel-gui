import { invoke } from "@tauri-apps/api/core";
import type { AppConfig, TunnelStatus } from "./types";

export async function loadConfig(): Promise<AppConfig> {
  return invoke<AppConfig>("load_config_cmd");
}

export async function saveConfig(config: AppConfig): Promise<void> {
  return invoke("save_config_cmd", { config });
}

export async function saveSecret(secret: string): Promise<void> {
  return invoke("save_secret_cmd", { secret });
}

export async function hasSecret(): Promise<boolean> {
  return invoke<boolean>("has_secret_cmd");
}

export async function startTunnel(config: AppConfig, secret: string): Promise<TunnelStatus> {
  return invoke<TunnelStatus>("start_tunnel", { config, secret });
}

export async function stopTunnel(): Promise<void> {
  return invoke("stop_tunnel");
}

export async function getStatus(): Promise<TunnelStatus> {
  return invoke<TunnelStatus>("get_status");
}

export async function copyAddress(): Promise<string> {
  return invoke<string>("copy_address");
}

export async function openConfigFolder(): Promise<void> {
  return invoke("open_config_folder");
}
