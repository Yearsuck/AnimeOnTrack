import { describe, it, expect } from "vitest";
import { countdown, formatProgress } from "./airCountdown";

describe("countdown", () => {
  const now = 1_000_000;
  const H = 3600;
  const M = 60;
  const D = 86400;

  it("must-pass: (now+1h45m) -> '1 h 45 min'", () => {
    expect(countdown(now + 1 * H + 45 * M, now)).toBe("1 h 45 min");
  });

  it("must-pass: (now+2d3h) -> '2 d 3 h'", () => {
    expect(countdown(now + 2 * D + 3 * H, now)).toBe("2 d 3 h");
  });

  it("must-pass: (now+30m) -> '30 min'", () => {
    expect(countdown(now + 30 * M, now)).toBe("30 min");
  });

  it("must-pass: (now-1m) -> null", () => {
    expect(countdown(now - 1 * M, now)).toBeNull();
  });

  it("returns null when airAt <= now or missing", () => {
    expect(countdown(now, now)).toBeNull();
    expect(countdown(null, now)).toBeNull();
    expect(countdown(undefined, now)).toBeNull();
  });

  it("floors everywhere (never rounds up)", () => {
    // 3 hours, 39 minutes, 11 seconds ahead -> "3 h 39 min" (not 40 min)
    expect(countdown(now + 13_151, now)).toBe("3 h 39 min");
    // 1 day, 3 hours, 39 minutes, 11 seconds ahead -> "1 d 3 h" (not 4 h)
    expect(countdown(now + 99_551, now)).toBe("1 d 3 h");
  });

  it("prefixes '≈' when estimated is true", () => {
    expect(countdown(now + 1 * H + 45 * M, now, { estimated: true })).toBe("≈ 1 h 45 min");
    expect(countdown(now + 2 * D + 3 * H, now, { estimated: true })).toBe("≈ 2 d 3 h");
    expect(countdown(now + 30 * M, now, { estimated: true })).toBe("≈ 30 min");
  });

  it("uses translation function t when provided", () => {
    const mockT = (key: string, params?: Record<string, string | number>) => {
      if (key === "catalog.countdownMinutes") return `${params?.m}m`;
      if (key === "catalog.countdownHours") return `${params?.h}h ${params?.m}m`;
      if (key === "catalog.countdownDays") return `${params?.d}d ${params?.h}h`;
      return "";
    };
    expect(countdown(now + 30 * M, now, { t: mockT })).toBe("30m");
    expect(countdown(now + 1 * H + 45 * M, now, { t: mockT })).toBe("1h 45m");
    expect(countdown(now + 2 * D + 3 * H, now, { t: mockT })).toBe("2d 3h");
  });
});

describe("formatProgress", () => {
  it("must-pass: progress(seen 4, aired 4, planned 12) -> '4 / 4 (12)'", () => {
    expect(formatProgress(4, 4, 12)).toBe("4 / 4 (12)");
  });

  it("must-pass: (4, 4, null) -> '4 / 4'", () => {
    expect(formatProgress(4, 4, null)).toBe("4 / 4");
  });

  it("must-pass: (4, 4, 4) -> '4 / 4'", () => {
    expect(formatProgress(4, 4, 4)).toBe("4 / 4");
  });

  it("handles missing planned (undefined)", () => {
    expect(formatProgress(4, 4)).toBe("4 / 4");
  });

  it("does not show parentheses when planned <= aired", () => {
    expect(formatProgress(5, 12, 10)).toBe("5 / 12");
  });

  it("shows planned when seen is 0 but planned > aired", () => {
    expect(formatProgress(0, 0, 12)).toBe("0 / 0 (12)");
  });
});
