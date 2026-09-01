import { describe, expect, it } from "vitest";
import {
  formatLastChecked,
  readLastChecked,
  writeLastChecked,
  type StorageLike,
} from "./updates";

const MIN = 60_000;
const HOUR = 60 * MIN;
const DAY = 24 * HOUR;

describe("formatLastChecked", () => {
  const now = 1_000_000_000_000;

  it("says never when there is no timestamp", () => {
    expect(formatLastChecked(null, now)).toBe("Never checked");
  });

  it("buckets into just-now / minutes / hours", () => {
    expect(formatLastChecked(now - 10_000, now)).toBe("Checked just now");
    expect(formatLastChecked(now - MIN, now)).toBe("Checked 1 minute ago");
    expect(formatLastChecked(now - 5 * MIN, now)).toBe("Checked 5 minutes ago");
    expect(formatLastChecked(now - HOUR, now)).toBe("Checked 1 hour ago");
    expect(formatLastChecked(now - 3 * HOUR, now)).toBe("Checked 3 hours ago");
  });

  it("falls back to a date once a day has passed", () => {
    const label = formatLastChecked(now - 2 * DAY, now);
    expect(label).toMatch(/^Checked on /);
  });

  it("never renders a negative age (clock skew)", () => {
    expect(formatLastChecked(now + HOUR, now)).toBe("Checked just now");
  });
});

/** Map-backed Storage fake for the node-env test run. */
function fakeStorage(initial = new Map<string, string>()): StorageLike {
  return {
    getItem: (k: string) => (initial.has(k) ? initial.get(k)! : null),
    setItem: (k: string, v: string) => void initial.set(k, v),
  };
}

describe("last-checked persistence", () => {
  it("roundtrips a timestamp", () => {
    const storage = fakeStorage();
    writeLastChecked(1234, storage);
    expect(readLastChecked(storage)).toBe(1234);
  });

  it("treats garbage, empty and missing as never-checked", () => {
    expect(readLastChecked(fakeStorage())).toBeNull();
    expect(readLastChecked(fakeStorage(new Map([[("sporeTunnel.lastUpdateCheck" as string), "yesterday"]])))).toBeNull();
    expect(readLastChecked(fakeStorage(new Map([[("sporeTunnel.lastUpdateCheck" as string), "0"]])))).toBeNull();
  });

  it("survives a throwing storage", () => {
    const broken: StorageLike = {
      getItem: () => {
        throw new Error("denied");
      },
      setItem: () => {
        throw new Error("denied");
      },
    };
    expect(readLastChecked(broken)).toBeNull();
    expect(() => writeLastChecked(1, broken)).not.toThrow();
  });
});
