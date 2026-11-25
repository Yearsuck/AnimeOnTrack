import { describe, it, expect } from "vitest";
import { progressPct, statusOf, progressRatio, pendingCount } from "./Library";
import type { LibraryItem } from "../types";

const item = (
  seen: number,
  total: number,
  airing = false,
  catalogEpisodes?: number | null,
  catalogStatus?: string | null
) =>
  ({
    seen_episodes: seen,
    total_episodes: total,
    watched_externally: false,
    series: { is_airing: airing },
    catalog_episodes: catalogEpisodes,
    catalog_status: catalogStatus,
  }) as unknown as LibraryItem;

describe("progressPct", () => {
  it("clamps to 0..100 and handles zero total", () => {
    expect(progressPct(15, 12)).toBe(100);
    expect(progressPct(-1, 12)).toBe(0);
    expect(progressPct(3, 0)).toBe(0);
    expect(progressPct(6, 12)).toBe(50);
  });
});

describe("statusOf", () => {
  it("seen > total on a finished series is completed", () => {
    expect(statusOf(item(13, 12))).toBe("completed");
    expect(statusOf(item(12, 12))).toBe("completed");
  });
  it("airing caught-up stays watching; zero seen is plan", () => {
    expect(statusOf(item(13, 12, true))).toBe("watching");
    expect(statusOf(item(0, 12))).toBe("plan");
  });
  it("partially scraped finished series stays watching when catalog says more episodes", () => {
    const itm = item(6, 6, false, 12, "FINISHED");
    expect(statusOf(itm)).toBe("watching");
    expect(progressRatio(itm)).toBe(0.5);
    expect(pendingCount(itm)).toBe(6);
  });
  it("fully scraped series whose site still says airing is completed when catalog says FINISHED", () => {
    const itm = item(12, 12, true, 12, "FINISHED");
    expect(statusOf(itm)).toBe("completed");
    expect(progressRatio(itm)).toBe(1);
    expect(pendingCount(itm)).toBe(0);
  });
  it("series without catalog fields falls back to site airing and total", () => {
    const itm = item(6, 6, false);
    expect(statusOf(itm)).toBe("completed");
    expect(progressRatio(itm)).toBe(1);
    expect(pendingCount(itm)).toBe(0);
  });
});
