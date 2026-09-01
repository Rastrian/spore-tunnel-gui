// The tunnel data store: zustand over the pure reducers in ./reducers.
// Fed exclusively by the three `tunnel://…` events plus one-shot hydration —
// never by polling.

import { create } from "zustand";
import {
  applyLogEvent,
  applyStatsEvent,
  applyStatusEvent,
  emptyTunnelData,
  removeProfile as removeProfileReducer,
  upsertProfile as upsertProfileReducer,
  type TunnelData,
} from "./reducers";
import type { LogEvent, Profile, StatsEvent, StatusEvent, UiPrefs } from "../lib/types";

interface TunnelStore extends TunnelData {
  applyStatus(ev: StatusEvent): void;
  applyLog(ev: LogEvent): void;
  applyStats(ev: StatsEvent): void;
  /** Replace the whole data snapshot (hydration). */
  replace(next: TunnelData): void;
  upsertProfile(profile: Profile): void;
  removeProfile(profileId: string): void;
  setUiPrefs(prefs: UiPrefs): void;
  setHasLegacy(hasLegacy: boolean): void;
}

export const useTunnels = create<TunnelStore>()((set) => ({
  ...emptyTunnelData,

  applyStatus: (ev) => set((s) => applyStatusEvent(s, ev)),
  applyLog: (ev) => set((s) => applyLogEvent(s, ev)),
  applyStats: (ev) => set((s) => applyStatsEvent(s, ev)),
  replace: (next) => set(next),
  upsertProfile: (profile) => set((s) => upsertProfileReducer(s, profile)),
  removeProfile: (profileId) => set((s) => removeProfileReducer(s, profileId)),
  setUiPrefs: (uiPrefs) => set({ uiPrefs }),
  setHasLegacy: (hasLegacy) => set({ hasLegacy }),
}));
