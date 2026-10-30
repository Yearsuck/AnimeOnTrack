import { describe, it, expect } from "vitest";
import { progressPct, statusOf } from "./Library";
import type { LibraryItem } from "../types";

const item = (seen: number, total: number, airing = false) =>
  ({ seen_episodes: seen, total_episodes: total, watched_externally: false, series: { is_airing: airing } }) as unknown as LibraryItem;

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
});
