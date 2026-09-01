// Pure reducers for the tunnel event store. No zustand, no Tauri, no React
// imports here — this module is the unit-test surface for every rule in
// docs/EVENTS.md (wholesale status replace, stats merge, log index/reset
// semantics, backfill composition, caps).

import type {
  LogEntry,
  LogEvent,
  Profile,
  ProfileStatus,
  StatsEvent,
  StatusEvent,
  TunnelStatus,
  UiPrefs,
} from "../lib/types";

/** Client-side log cap per profile (backend ring keeps 1024). */
export const MAX_LOG_LINES = 1000;

/** Throughput sparkline window, in per-second samples. */
export const MAX_SAMPLES = 60;

/** One per-second throughput sample (delta of the cumulative counters). */
export interface ThroughputSample {
  up: number;
  down: number;
}

export interface TunnelData {
  profiles: Profile[];
  statuses: Record<string, TunnelStatus>;
  logs: Record<string, LogEntry[]>;
  samples: Record<string, ThroughputSample[]>;
  hasLegacy: boolean;
  uiPrefs: UiPrefs | null;
  /** Hydration finished (safe to show the wizard / pick a selection). */
  hydrated: boolean;
}

export const emptyTunnelData: TunnelData = {
  profiles: [],
  statuses: {},
  logs: {},
  samples: {},
  hasLegacy: false,
  uiPrefs: null,
  hydrated: false,
};

/** `tunnel://status` — full snapshot, replace wholesale. */
export function applyStatusEvent(
  state: TunnelData,
  { profileId, status }: StatusEvent,
): TunnelData {
  const next: TunnelData = {
    ...state,
    statuses: { ...state.statuses, [profileId]: status },
  };
  // A non-running tunnel has no meaningful throughput history; a run that is
  // about to start will restart its counters from zero.
  if (status.state === "idle") {
    next.samples = { ...state.samples, [profileId]: [] };
  }
  return next;
}

/**
 * `tunnel://stats` — merge into an existing status only. A stats event for an
 * unknown tunnel is dropped (hydration supplies the status snapshot).
 */
export function applyStatsEvent(state: TunnelData, ev: StatsEvent): TunnelData {
  const status = state.statuses[ev.profileId];
  if (!status) return state;

  // Deltas come from the cumulative counters; a counter going backwards means
  // a new run began (counters restart at 0) — reset the sparkline window.
  const dUp = ev.bytesUp - status.bytesUp;
  const dDown = ev.bytesDown - status.bytesDown;
  const reset = dUp < 0 || dDown < 0;
  const sample: ThroughputSample = {
    up: Math.max(0, dUp),
    down: Math.max(0, dDown),
  };
  const prev = reset ? [] : (state.samples[ev.profileId] ?? []);
  const samples = { ...state.samples, [ev.profileId]: [...prev, sample].slice(-MAX_SAMPLES) };

  return {
    ...state,
    statuses: {
      ...state.statuses,
      [ev.profileId]: {
        ...status,
        bytesUp: ev.bytesUp,
        bytesDown: ev.bytesDown,
        uptimeSecs: ev.uptimeSecs,
      },
    },
    samples,
  };
}

/**
 * `tunnel://log` — append with the strictly-increasing index rule:
 * equal index = duplicate, skip; smaller index = a new run started
 * (`start_tunnel` resets the ring), drop the buffered run and start over.
 */
export function applyLogEvent(
  state: TunnelData,
  { profileId, index, line, level, ts }: LogEvent,
): TunnelData {
  const buffered = state.logs[profileId] ?? [];
  const last = buffered.length ? buffered[buffered.length - 1].index : -1;
  const entry: LogEntry = { index, ts, level, line };

  if (index === last) return state; // duplicate delivery
  if (index < last) {
    // New run: the old one is gone.
    return { ...state, logs: { ...state.logs, [profileId]: [entry] } };
  }
  return {
    ...state,
    logs: {
      ...state.logs,
      [profileId]: [...buffered, entry].slice(-MAX_LOG_LINES),
    },
  };
}

/**
 * Merge a `get_tunnel_log` backfill with the entries already buffered from
 * live events (which may have arrived between subscribing and the backfill
 * resolving). Indexes are unique within a run, so the two compose by
 * strictly-greater-than:
 *
 * - buffered tail beyond the backfill tail (gap or overlap) → append it,
 * - buffered entirely inside the backfill → same run, backfill wins,
 * - buffered behind the backfill tail but not contained in it → the live
 *   stream already moved to a new run; keep the buffered (new) run only.
 */
export function mergeLogRuns(
  backfill: LogEntry[],
  buffered: LogEntry[],
): LogEntry[] {
  if (backfill.length === 0) return buffered;
  if (buffered.length === 0) return backfill;
  const backfillLast = backfill[backfill.length - 1].index;
  const bufferedLast = buffered[buffered.length - 1].index;

  if (bufferedLast > backfillLast) {
    const tail = buffered.filter((e) => e.index > backfillLast);
    return [...backfill, ...tail];
  }
  const known = new Set(backfill.map((e) => e.index));
  const contained = buffered.every((e) => known.has(e.index));
  return contained ? backfill : buffered;
}

export interface HydrationInput {
  profiles: Profile[];
  allStatus: ProfileStatus[];
  /** Full backfill per profile, as returned by `get_tunnel_log`. */
  backfill: Record<string, LogEntry[]>;
  hasLegacy: boolean;
  uiPrefs: UiPrefs | null;
}

/**
 * One-shot hydration (runs after the event subscriptions are in place). A
 * status an event already delivered is strictly fresher than the snapshot,
 * so it wins; logs compose via `mergeLogRuns`.
 */
export function hydrate(state: TunnelData, input: HydrationInput): TunnelData {
  const statuses = { ...state.statuses };
  for (const { profileId, status } of input.allStatus) {
    if (!statuses[profileId]) statuses[profileId] = status;
  }

  const logs: Record<string, LogEntry[]> = { ...state.logs };
  for (const [profileId, entries] of Object.entries(input.backfill)) {
    logs[profileId] = mergeLogRuns(entries, state.logs[profileId] ?? []).slice(
      -MAX_LOG_LINES,
    );
  }

  return {
    ...state,
    profiles: input.profiles,
    statuses,
    logs,
    hasLegacy: input.hasLegacy,
    uiPrefs: input.uiPrefs ?? state.uiPrefs,
    hydrated: true,
  };
}

/** Insert or update a profile after save_profile / import_legacy. */
export function upsertProfile(state: TunnelData, profile: Profile): TunnelData {
  const exists = state.profiles.some((p) => p.id === profile.id);
  return {
    ...state,
    profiles: exists
      ? state.profiles.map((p) => (p.id === profile.id ? profile : p))
      : [...state.profiles, profile],
  };
}

/** Drop a profile (and its tunnel view data) after delete_profile. */
export function removeProfile(state: TunnelData, profileId: string): TunnelData {
  const profiles = state.profiles.filter((p) => p.id !== profileId);
  const statuses = { ...state.statuses };
  const logs = { ...state.logs };
  const samples = { ...state.samples };
  delete statuses[profileId];
  delete logs[profileId];
  delete samples[profileId];
  return { ...state, profiles, statuses, logs, samples };
}
