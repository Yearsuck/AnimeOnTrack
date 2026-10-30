import { describe, it, expect } from "vitest";
import { getMonthBoundaries, intensityThresholds, getIntensityLevel, type HeatmapWeek } from "./heatmapLayout";

function week(weekIndex: number, start: string): HeatmapWeek {
  const [y, m, d] = start.split("-").map(Number);
  const days = Array.from({ length: 7 }, (_, i) => {
    const dt = new Date(Date.UTC(y, m - 1, d + i));
    return { date: dt.toISOString().slice(0, 10), count: 0, level: 0 as const, dayOfWeek: i };
  });
  return { weekIndex, days };
}

describe("getMonthBoundaries", () => {
  it("places a month on the week containing its 1st, even mid-week", () => {
    const weeks = [week(0, "2026-01-19"), week(1, "2026-01-26"), week(2, "2026-02-02")];
    const b = getMonthBoundaries(weeks);
    expect(b).toEqual([
      { month: 0, weekIndex: 0 },
      { month: 1, weekIndex: 1 },
    ]);
  });
});

describe("intensity", () => {
  const lvl = (counts: number[], c: number) => getIntensityLevel(c, intensityThresholds(counts));
  it("single active day reaches level 4", () => {
    expect(lvl([10], 10)).toBe(4);
    expect(lvl([1], 1)).toBe(4);
  });
  it("two days: busiest is 4, other lower", () => {
    expect(lvl([2, 10], 10)).toBe(4);
    expect(lvl([2, 10], 2)).toBe(1);
  });
  it("thresholds stay monotonic and max is level 4 on a normal case", () => {
    const counts = [1, 2, 3, 4, 5, 6, 7, 8];
    const [a, b, c] = intensityThresholds(counts);
    expect(a <= b && b <= c).toBe(true);
    expect(lvl(counts, 8)).toBe(4);
    expect(lvl(counts, 1)).toBe(1);
    expect(lvl(counts, 0)).toBe(0);
  });
  it("empty keeps defaults", () => {
    expect(intensityThresholds([0, 0])).toEqual([1, 2, 3]);
  });
});
