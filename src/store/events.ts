// Event wiring: subscribe to the three `tunnel://…` events FIRST, then run
// the one-shot hydration commands (docs/EVENTS.md startup order). The init
// promise is cached at module level so React StrictMode's double effect
// mount cannot subscribe twice.

import { listen } from "@tauri-apps/api/event";
import {
  getAllStatus,
  getTunnelLog,
  getUiPrefs,
  hasLegacyConfig,
  listProfiles,
} from "../lib/api";
import type {
  LogEntry,
  LogEvent,
  Profile,
  ProfileStatus,
  StatsEvent,
  StatusEvent,
  UiPrefs,
} from "../lib/types";
import { useTunnels } from "./tunnels";
import { hydrate, type TunnelData } from "./reducers";

let initPromise: Promise<void> | null = null;

/**
 * Idempotent initializer. Resolves once subscriptions are live and hydration
 * has landed in the store. A failure resets the cache so a later mount can
 * retry (e.g. the webview was not ready yet).
 */
export function initTunnelEvents(): Promise<void> {
  if (!initPromise) {
    initPromise = doInit().catch((err) => {
      initPromise = null;
      throw err;
    });
  }
  return initPromise;
}

async function doInit(): Promise<void> {
  // Subscriptions live for the lifetime of the webview; nothing unlistens.
  await Promise.all([
    listen<StatusEvent>("tunnel://status", ({ payload }) =>
      useTunnels.getState().applyStatus(payload),
    ),
    listen<LogEvent>("tunnel://log", ({ payload }) =>
      useTunnels.getState().applyLog(payload),
    ),
    listen<StatsEvent>("tunnel://stats", ({ payload }) =>
      useTunnels.getState().applyStats(payload),
    ),
  ]);

  // One-shot hydration AFTER subscribing, so nothing emitted in between is
  // lost. Each piece fails independently (first hydration is best-effort,
  // like the legacy app's load()); defaults leave a coherent empty store.
  const [profiles, allStatus, hasLegacy, uiPrefs] = await Promise.all([
    listProfiles().catch((err: unknown) => {
      console.error("list_profiles failed", err);
      return [] as Profile[];
    }),
    getAllStatus().catch((err: unknown) => {
      console.error("get_all_status failed", err);
      return [] as ProfileStatus[];
    }),
    hasLegacyConfig().catch((err: unknown) => {
      console.error("has_legacy_config failed", err);
      return false;
    }),
    getUiPrefs().catch((err: unknown) => {
      console.error("get_ui_prefs failed", err);
      return null as UiPrefs | null;
    }),
  ]);

  const backfill: Record<string, LogEntry[]> = {};
  for (const { profileId } of allStatus) {
    try {
      backfill[profileId] = await getTunnelLog(profileId, undefined);
    } catch (err) {
      console.error(`get_tunnel_log(${profileId}) failed`, err);
    }
  }

  const next: TunnelData = hydrate(useTunnels.getState(), {
    profiles,
    allStatus,
    backfill,
    hasLegacy,
    uiPrefs,
  });
  useTunnels.getState().replace(next);
}
