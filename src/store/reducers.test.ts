import { describe, expect, it } from "vitest";
import {
  MAX_LOG_LINES,
  applyLogEvent,
  applyStatsEvent,
  applyStatusEvent,
  emptyTunnelData,
  hydrate,
  mergeLogRuns,
  upsertProfile,
  removeProfile,
  type TunnelData,
} from "./reducers";
import type {
  LogEntry,
  LogEvent,
  Profile,
  ProfileStatus,
  StatsEvent,
  TunnelStatus,
} from "../lib/types";

function status(overrides: Partial<TunnelStatus> = {}): TunnelStatus {
  return {
    state: "idle",
    serverKind: null,
    localAddress: "127.0.0.1:25565",
    remoteAddress: null,
    assignedRemotePort: null,
    uptimeSecs: 0,
    bytesUp: 0,
    bytesDown: 0,
    reconnects: 0,
    lastError: null,
    logs: [],
    ...overrides,
  };
}

function profile(id = "p1"): Profile {
  return {
    id,
    name: "Tunnel",
    serverHost: "spore.example.com",
    serverPort: 7835,
    localHost: "127.0.0.1",
    localPort: 25565,
    remotePort: 0,
    autostart: false,
    autoReconnect: true,
  };
}

function logEvent(index: number, line = `line ${index}`, profileId = "p1"): LogEvent {
  return { profileId, index, line, level: "info", ts: 1_700_000_000_000 + index };
}

function entry(index: number, line = `line ${index}`): LogEntry {
  return { index, line, level: "info", ts: 1_700_000_000_000 + index };
}

describe("applyStatusEvent", () => {
  it("replaces the previous status wholesale", () => {
    let state: TunnelData = applyStatusEvent(emptyTunnelData, {
      profileId: "p1",
      status: status({ state: "connected", remoteAddress: "a.example.com:1000", bytesUp: 5 }),
    });
    state = applyStatusEvent(state, {
      profileId: "p1",
      // Stale-looking values must vanish: replace, not merge.
      status: status({ state: "failed", lastError: "boom", bytesUp: 0 }),
    });

    expect(state.statuses["p1"]).toEqual(status({ state: "failed", lastError: "boom" }));
    expect(Object.keys(state.statuses)).toEqual(["p1"]);
  });

  it("does not touch other profiles", () => {
    let state = applyStatusEvent(emptyTunnelData, {
      profileId: "a",
      status: status({ state: "connected" }),
    });
    state = applyStatusEvent(state, { profileId: "b", status: status({ state: "idle" }) });
    expect(state.statuses["a"]?.state).toBe("connected");
    expect(state.statuses["b"]?.state).toBe("idle");
  });

  it("clears the throughput samples when the tunnel goes idle", () => {
    let state: TunnelData = emptyTunnelData;
    state = applyStatusEvent(state, { profileId: "p1", status: status({ state: "connected" }) });
    state = applyStatsEvent(state, { profileId: "p1", bytesUp: 100, bytesDown: 50, uptimeSecs: 1 });
    expect(state.samples["p1"]).toHaveLength(1);

    state = applyStatusEvent(state, { profileId: "p1", status: status({ state: "idle" }) });
    expect(state.samples["p1"]).toEqual([]);
  });
});

describe("applyStatsEvent", () => {
  it("is ignored when no status exists yet (hydration supplies it)", () => {
    const ev: StatsEvent = { profileId: "ghost", bytesUp: 10, bytesDown: 10, uptimeSecs: 1 };
    expect(applyStatsEvent(emptyTunnelData, ev)).toBe(emptyTunnelData);
  });

  it("merges counters into an existing status", () => {
    let state = applyStatusEvent(emptyTunnelData, {
      profileId: "p1",
      status: status({ state: "connected", bytesUp: 100, bytesDown: 40, uptimeSecs: 3 }),
    });
    state = applyStatsEvent(state, { profileId: "p1", bytesUp: 300, bytesDown: 90, uptimeSecs: 4 });

    expect(state.statuses["p1"]).toMatchObject({ bytesUp: 300, bytesDown: 90, uptimeSecs: 4 });
  });

  it("records per-second deltas and resets on counter regression (new run)", () => {
    let state = applyStatusEvent(emptyTunnelData, {
      profileId: "p1",
      status: status({ state: "connected", bytesUp: 100, bytesDown: 100 }),
    });
    state = applyStatsEvent(state, { profileId: "p1", bytesUp: 150, bytesDown: 120, uptimeSecs: 1 });
    expect(state.samples["p1"]).toEqual([{ up: 50, down: 20 }]);

    state = applyStatsEvent(state, { profileId: "p1", bytesUp: 200, bytesDown: 140, uptimeSecs: 2 });
    expect(state.samples["p1"]).toEqual([
      { up: 50, down: 20 },
      { up: 50, down: 20 },
    ]);

    // Counters restarted at ~0 for a new run: the window resets and the
    // regressed delta is clamped away (no bogus negative sample).
    state = applyStatsEvent(state, { profileId: "p1", bytesUp: 5, bytesDown: 2, uptimeSecs: 1 });
    expect(state.samples["p1"]).toEqual([{ up: 0, down: 0 }]);
    state = applyStatsEvent(state, { profileId: "p1", bytesUp: 25, bytesDown: 9, uptimeSecs: 2 });
    expect(state.samples["p1"]).toEqual([
      { up: 0, down: 0 },
      { up: 20, down: 7 },
    ]);
  });
});

describe("applyLogEvent", () => {
  it("appends entries with increasing indexes", () => {
    let state = applyLogEvent(emptyTunnelData, logEvent(0));
    state = applyLogEvent(state, logEvent(1));
    state = applyLogEvent(state, logEvent(2));
    expect(state.logs["p1"].map((e) => e.index)).toEqual([0, 1, 2]);
  });

  it("skips a duplicate (equal index) delivery", () => {
    let state = applyLogEvent(emptyTunnelData, logEvent(0));
    state = applyLogEvent(state, logEvent(0));
    expect(state.logs["p1"]).toHaveLength(1);
  });

  it("drops the whole buffered run when the index resets (restart)", () => {
    let state = applyLogEvent(emptyTunnelData, logEvent(4));
    state = applyLogEvent(state, logEvent(5));
    state = applyLogEvent(state, logEvent(6));
    state = applyLogEvent(state, logEvent(0, "fresh run"));
    expect(state.logs["p1"]).toEqual([entry(0, "fresh run")]);

    state = applyLogEvent(state, logEvent(1, "fresh run 2"));
    expect(state.logs["p1"].map((e) => e.line)).toEqual(["fresh run", "fresh run 2"]);
  });

  it("keeps only the last MAX_LOG_LINES entries", () => {
    let state = emptyTunnelData;
    for (let i = 0; i < MAX_LOG_LINES + 250; i++) {
      state = applyLogEvent(state, logEvent(i));
    }
    const buffered = state.logs["p1"];
    expect(buffered).toHaveLength(MAX_LOG_LINES);
    expect(buffered[0].index).toBe(250);
    expect(buffered[buffered.length - 1].index).toBe(MAX_LOG_LINES + 249);
  });
});

describe("mergeLogRuns (backfill + live buffer)", () => {
  it("concatenates when the live tail is ahead of the backfill", () => {
    const backfill = [entry(1), entry(2), entry(3)];
    const buffered = [entry(4), entry(5)];
    expect(mergeLogRuns(backfill, buffered).map((e) => e.index)).toEqual([1, 2, 3, 4, 5]);
  });

  it("dedupes the overlap without gaps", () => {
    // Live events 2 and 3 arrived before the backfill resolved.
    const backfill = [entry(1), entry(2), entry(3), entry(4)];
    const buffered = [entry(2), entry(3)];
    expect(mergeLogRuns(backfill, buffered).map((e) => e.index)).toEqual([1, 2, 3, 4]);
  });

  it("bridges ring-eviction gaps by index, not contiguity", () => {
    const backfill = [entry(1), entry(2)];
    const buffered = [entry(9), entry(10)]; // 3..8 were evicted server-side
    expect(mergeLogRuns(backfill, buffered).map((e) => e.index)).toEqual([1, 2, 9, 10]);
  });

  it("keeps the buffered new run when a restart raced the backfill", () => {
    // Backfill still holds the tail of the dead run; the live stream is
    // already on a fresh run with lower indexes.
    const backfill = [entry(7), entry(8), entry(9)];
    const buffered = [entry(0, "new"), entry(1, "new")];
    expect(mergeLogRuns(backfill, buffered).map((e) => e.line)).toEqual(["new", "new"]);
  });

  it("passes through either empty side", () => {
    expect(mergeLogRuns([], [entry(1)])).toEqual([entry(1)]);
    expect(mergeLogRuns([entry(1)], [])).toEqual([entry(1)]);
  });
});

describe("hydrate", () => {
  const allStatus: ProfileStatus[] = [
    { profileId: "p1", status: status({ state: "connected", remoteAddress: "srv:1" }) },
  ];

  it("lands profiles, statuses, prefs and the hydrated flag", () => {
    const state = hydrate(emptyTunnelData, {
      profiles: [profile()],
      allStatus,
      backfill: { p1: [entry(0)] },
      hasLegacy: true,
      uiPrefs: { theme: "light", startMinimized: false, closeToTray: false },
    });

    expect(state.profiles).toHaveLength(1);
    expect(state.statuses["p1"].state).toBe("connected");
    expect(state.logs["p1"]).toEqual([entry(0)]);
    expect(state.hasLegacy).toBe(true);
    expect(state.uiPrefs?.theme).toBe("light");
    expect(state.hydrated).toBe(true);
  });

  it("composes the backfill with live-buffered logs without duplicates or gaps", () => {
    // Live events captured between subscribe and backfill resolution.
    let live = applyLogEvent(emptyTunnelData, logEvent(2));
    live = applyLogEvent(live, logEvent(3, "live"));
    live = applyLogEvent(live, logEvent(4));

    const state = hydrate(live, {
      profiles: [profile()],
      allStatus,
      backfill: { p1: [entry(0), entry(1), entry(2), entry(3)] },
      hasLegacy: false,
      uiPrefs: null,
    });

    expect(state.logs["p1"].map((e) => e.index)).toEqual([0, 1, 2, 3, 4]);
  });

  it("prefers a fresher event-delivered status over the snapshot", () => {
    const live = applyStatusEvent(emptyTunnelData, {
      profileId: "p1",
      status: status({ state: "connected", uptimeSecs: 99 }),
    });
    const stale: ProfileStatus[] = [
      { profileId: "p1", status: status({ state: "starting", uptimeSecs: 1 }) },
    ];
    const state = hydrate(live, {
      profiles: [profile()],
      allStatus: stale,
      backfill: {},
      hasLegacy: false,
      uiPrefs: null,
    });
    expect(state.statuses["p1"].state).toBe("connected");
  });

  it("caps the composed log at MAX_LOG_LINES", () => {
    const backfill: LogEntry[] = Array.from({ length: MAX_LOG_LINES }, (_, i) => entry(i));
    const buffered = [entry(MAX_LOG_LINES), entry(MAX_LOG_LINES + 1)];
    const state = hydrate(
      { ...emptyTunnelData, logs: { p1: buffered } },
      {
        profiles: [profile()],
        allStatus,
        backfill: { p1: backfill },
        hasLegacy: false,
        uiPrefs: null,
      },
    );
    expect(state.logs["p1"]).toHaveLength(MAX_LOG_LINES);
    expect(state.logs["p1"][0].index).toBe(2);
  });
});

describe("profile list reducers", () => {
  it("upsertProfile inserts then updates in place", () => {
    let state = upsertProfile(emptyTunnelData, profile("a"));
    state = upsertProfile(state, profile("b"));
    state = upsertProfile(state, { ...profile("a"), name: "Renamed" });
    expect(state.profiles.map((p) => p.name)).toEqual(["Renamed", "Tunnel"]);
  });

  it("removeProfile drops the profile and its tunnel data", () => {
    let state = upsertProfile(emptyTunnelData, profile("a"));
    state = applyStatusEvent(state, { profileId: "a", status: status() });
    state = applyLogEvent(state, logEvent(0, "x", "a"));
    state = applyStatsEvent(state, { profileId: "a", bytesUp: 1, bytesDown: 1, uptimeSecs: 1 });

    state = removeProfile(state, "a");
    expect(state.profiles).toEqual([]);
    expect(state.statuses["a"]).toBeUndefined();
    expect(state.logs["a"]).toBeUndefined();
    expect(state.samples["a"]).toBeUndefined();
  });
});
