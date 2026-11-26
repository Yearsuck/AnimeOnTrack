import { describe, it, expect, afterEach, vi } from "vitest";
import { isWithinLastThreeMonths } from "./AiringGrid";

const sec = (d: Date) => d.getTime() / 1000;
afterEach(() => vi.useRealTimers());

describe("isWithinLastThreeMonths", () => {
  it("May 31 -> cutoff Feb 28 (2026)", () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date(2026, 4, 31, 12));
    expect(isWithinLastThreeMonths(sec(new Date(2026, 1, 28, 13)))).toBe(true);
    expect(isWithinLastThreeMonths(sec(new Date(2026, 1, 27, 12)))).toBe(false);
  });
  it("May 31 leap year -> cutoff Feb 29 (2028)", () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date(2028, 4, 31, 12));
    expect(isWithinLastThreeMonths(sec(new Date(2028, 1, 29, 13)))).toBe(true);
    expect(isWithinLastThreeMonths(sec(new Date(2028, 1, 28, 12)))).toBe(false);
  });
  it("Dec 31 -> cutoff Sep 30", () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date(2026, 11, 31, 12));
    expect(isWithinLastThreeMonths(sec(new Date(2026, 8, 30, 13)))).toBe(true);
    expect(isWithinLastThreeMonths(sec(new Date(2026, 8, 29, 12)))).toBe(false);
  });
  it("Jan 15 crosses the year", () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date(2026, 0, 15, 12));
    expect(isWithinLastThreeMonths(sec(new Date(2025, 9, 15, 13)))).toBe(true);
    expect(isWithinLastThreeMonths(sec(new Date(2025, 9, 14, 12)))).toBe(false);
  });
});

import { sortAiringItems } from "./AiringGrid";
import type { AiringItem } from "../types";

const mk = (id: number, title: string, score: number | null): AiringItem =>
  ({ series: { id, title, next_episode_at: 100 - id }, score }) as unknown as AiringItem;

describe("sortAiringItems", () => {
  const items = [mk(1, "A", 70), mk(2, "B", null), mk(3, "C", 90), mk(4, "D", 70), mk(5, "E", 0)];
  it("release keeps the incoming order", () => {
    expect(sortAiringItems(items, "release").map((i) => i.series.id)).toEqual([1, 2, 3, 4, 5]);
  });
  it("score sorts desc, ties keep order, missing scores go last", () => {
    expect(sortAiringItems(items, "score").map((i) => i.series.id)).toEqual([3, 1, 4, 5, 2]);
  });
});
