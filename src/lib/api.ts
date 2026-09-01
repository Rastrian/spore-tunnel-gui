// Typed wrappers for every Tauri command. Tauri v2 maps camelCase JS
// argument keys to the snake_case Rust parameters.

import { invoke } from "@tauri-apps/api/core";
import type { LogEntry, Profile, ProfileStatus, TunnelStatus } from "./types";

export function listProfiles(): Promise<Profile[]> {
  return invoke<Profile[]>("list_profiles");
}

export function saveProfile(profile: Profile): Promise<Profile> {
  return invoke<Profile>("save_profile", { profile });
}

export function setActiveProfile(profileId: string): Promise<void> {
  return invoke("set_active_profile", { profileId });
}

export function setProfileSecret(profileId: string, secret: string): Promise<void> {
  return invoke("set_profile_secret", { profileId, secret });
}

export function deleteProfile(profileId: string): Promise<void> {
  return invoke("delete_profile", { profileId });
}

export function importLegacy(): Promise<Profile | null> {
  return invoke<Profile | null>("import_legacy");
}

export function hasLegacyConfig(): Promise<boolean> {
  return invoke<boolean>("has_legacy_config");
}

export function startTunnel(profileId: string, secret?: string): Promise<TunnelStatus> {
  return invoke<TunnelStatus>("start_tunnel", { profileId, secret });
}

/** Omit `profileId` to target the active profile. */
export function stopTunnel(profileId?: string): Promise<void> {
  return invoke("stop_tunnel", { profileId });
}

export function getStatus(profileId?: string): Promise<TunnelStatus> {
  return invoke<TunnelStatus>("get_status", { profileId });
}

export function getAllStatus(): Promise<ProfileStatus[]> {
  return invoke<ProfileStatus[]>("get_all_status");
}

export function getTunnelLog(profileId: string, sinceIndex?: number): Promise<LogEntry[]> {
  return invoke<LogEntry[]>("get_tunnel_log", { profileId, sinceIndex });
}

export function copyAddress(profileId?: string): Promise<string> {
  return invoke<string>("copy_address", { profileId });
}

export function openConfigFolder(): Promise<void> {
  return invoke("open_config_folder");
}
