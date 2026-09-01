import { describe, expect, it } from "vitest";
import { formatClock, formatRate, formatUptime, humanizeBytes } from "./format";

describe("humanizeBytes", () => {
  it("renders sub-KiB values as plain bytes", () => {
    expect(humanizeBytes(0)).toBe("0 B");
    expect(humanizeBytes(1)).toBe("1 B");
    expect(humanizeBytes(999)).toBe("999 B");
    expect(humanizeBytes(1023)).toBe("1023 B");
  });

  it("renders exact and fractional KiB/MiB/GiB", () => {
    expect(humanizeBytes(1024)).toBe("1.0 KiB");
    expect(humanizeBytes(1536)).toBe("1.5 KiB");
    expect(humanizeBytes(1_048_576)).toBe("1.0 MiB");
    expect(humanizeBytes(1_048_576 * 1.5)).toBe("1.5 MiB");
    expect(humanizeBytes(1_073_741_824)).toBe("1.0 GiB");
  });

  it("drops the decimal once the number is wide", () => {
    expect(humanizeBytes(1024 * 12.34)).toBe("12 KiB");
    expect(humanizeBytes(1024 * 123.4)).toBe("123 KiB");
  });

  it("never shows negative values (counter races)", () => {
    expect(humanizeBytes(-5)).toBe("0 B");
  });
});

describe("formatUptime", () => {
  it("formats mm:ss under an hour", () => {
    expect(formatUptime(0)).toBe("00:00");
    expect(formatUptime(5)).toBe("00:05");
    expect(formatUptime(65)).toBe("01:05");
    expect(formatUptime(3599)).toBe("59:59");
  });

  it("formats h:mm:ss at an hour and beyond", () => {
    expect(formatUptime(3600)).toBe("1:00:00");
    expect(formatUptime(3661)).toBe("1:01:01");
    expect(formatUptime(75_510)).toBe("20:58:30");
  });

  it("clamps negatives to zero", () => {
    expect(formatUptime(-3)).toBe("00:00");
  });
});

describe("formatRate", () => {
  it("appends the per-second unit", () => {
    expect(formatRate(0)).toBe("0 B/s");
    expect(formatRate(2048)).toBe("2.0 KiB/s");
  });
});

describe("formatClock", () => {
  it("renders local HH:MM:SS with zero padding", () => {
    const ts = new Date(2026, 0, 2, 3, 4, 5).getTime();
    expect(formatClock(ts)).toBe("03:04:05");
    expect(formatClock(new Date(2026, 0, 2, 23, 0, 9).getTime())).toBe("23:00:09");
  });
});
