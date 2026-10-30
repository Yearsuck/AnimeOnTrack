import { describe, it, expect } from "vitest";
import { formatMinutes } from "./StatsInsights";

const t = ((k: string, p?: Record<string, number>) => {
  if (k === "stats.minutesUnit") return `${p!.minutes} min`;
  if (k === "stats.hoursUnit") return `${p!.hours} h`;
  if (k === "stats.daysUnit") return `${p!.days} d ${p!.hours} h`;
  return k;
}) as Parameters<typeof formatMinutes>[0];

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
