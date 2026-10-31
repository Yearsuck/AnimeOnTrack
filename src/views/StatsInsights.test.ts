import { describe, it, expect } from "vitest";
import {
  formatMinutes,
  formatTimeWatched,
  formatTimeWatchedHelp,
  formatEpisodesWatchedNote,
  formatPopularityPercentile,
} from "./StatsInsights";

const t = ((k: string, p?: Record<string, string | number>) => {
  if (k === "stats.minutesUnit") return `${p!.minutes} min`;
  if (k === "stats.hoursUnit") return `${p!.hours} h`;
  if (k === "stats.daysUnit") return `${p!.days} d ${p!.hours} h`;
  if (k === "stats.timeWatchedHelp") return `${p!.pct} % estimado con duraciones por defecto`;
  if (k === "stats.timeWatchedExact") return "duraciones de AniList";
  if (k === "stats.episodesWatchedHelp") return `+${p!.external} de «Ya lo vi»`;
  if (k === "stats.popularityPercentile") return `más popular que el ${p!.pct} % del catálogo`;
  return k;
}) as Parameters<typeof formatMinutes>[0];

const n = (v: number) => String(v);
const fd = (v: number) => v.toFixed(1);

describe("formatMinutes", () => {
  it("rounds consistently across unit boundaries", () => {
    expect(formatMinutes(t, 59.6)).toBe("1 h");
    expect(formatMinutes(t, 29)).toBe("29 min");
    expect(formatMinutes(t, 45)).toBe("45 min");
    expect(formatMinutes(t, 89)).toBe("1 h");
    expect(formatMinutes(t, 2879)).toBe("2 d 0 h");
    expect(formatMinutes(t, 2880)).toBe("2 d 0 h");
    expect(formatMinutes(t, 2820)).toBe("47 h");
  });
  it("treats NaN/negative as 0", () => {
    expect(formatMinutes(t, NaN)).toBe("0 min");
    expect(formatMinutes(t, -5)).toBe("0 min");
  });
});

describe("formatTimeWatched", () => {
  it("prepends ≈ when estimated_minutes_share > 0", () => {
    expect(formatTimeWatched(t, 70, 0.34)).toBe("≈ 1 h");
    expect(formatTimeWatched(t, 120, 1.0)).toBe("≈ 2 h");
  });

  it("omits ≈ when estimated_minutes_share is 0", () => {
    expect(formatTimeWatched(t, 70, 0.0)).toBe("1 h");
    expect(formatTimeWatched(t, 120, 0)).toBe("2 h");
  });
});

describe("formatTimeWatchedHelp", () => {
  it("reports percentage estimated when share > 0", () => {
    expect(formatTimeWatchedHelp(t, 0.3428, n)).toBe("34 % estimado con duraciones por defecto");
    expect(formatTimeWatchedHelp(t, 1.0, n)).toBe("100 % estimado con duraciones por defecto");
  });

  it("never reads 0 % next to an approximate figure", () => {
    expect(formatTimeWatchedHelp(t, 0.004, n)).toBe("< 1 % estimado con duraciones por defecto");
  });

  it("reports exact AniList durations when share is 0", () => {
    expect(formatTimeWatchedHelp(t, 0.0, n)).toBe("duraciones de AniList");
  });
});

describe("formatEpisodesWatchedNote", () => {
  it("returns secondary note when external > 0", () => {
    expect(formatEpisodesWatchedNote(t, 12, n)).toBe("+12 de «Ya lo vi»");
  });

  it("returns null when external is 0", () => {
    expect(formatEpisodesWatchedNote(t, 0, n)).toBeNull();
  });
});

describe("formatPopularityPercentile", () => {
  it("returns em-dash when percentile is null", () => {
    expect(formatPopularityPercentile(t, null, n, fd)).toBe("—");
  });

  it("formats integer percentile without decimals", () => {
    expect(formatPopularityPercentile(t, 50, n, fd)).toBe("más popular que el 50 % del catálogo");
  });

  it("formats fractional percentile with decimals", () => {
    expect(formatPopularityPercentile(t, 50.4, n, fd)).toBe("más popular que el 50.4 % del catálogo");
  });
});
