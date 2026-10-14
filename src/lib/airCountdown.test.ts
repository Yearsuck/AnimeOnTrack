import { describe, it, expect } from "vitest";
import { airCountdown } from "./airCountdown";

describe("airCountdown", () => {
  const now = 1_000_000;

  it("returns { unit: 'minutes', value: 30 } when 30 min ahead", () => {
    expect(airCountdown(now + 30 * 60, now)).toEqual({
      unit: "minutes",
      value: 30,
    });
  });

  it("returns { unit: 'minutes', value: 1 } when 1 s ahead", () => {
    expect(airCountdown(now + 1, now)).toEqual({
      unit: "minutes",
      value: 1,
    });
  });

  it("returns { unit: 'hours', value: 5 } when 5 h ahead", () => {
    expect(airCountdown(now + 5 * 3600, now)).toEqual({
      unit: "hours",
      value: 5,
    });
  });

  it("returns { unit: 'hours', value: 47 } when 47 h ahead", () => {
    expect(airCountdown(now + 47 * 3600, now)).toEqual({
      unit: "hours",
      value: 47,
    });
  });

  it("returns { unit: 'days', value: 2 } when 49 h ahead", () => {
    expect(airCountdown(now + 49 * 3600, now)).toEqual({
      unit: "days",
      value: 2,
    });
  });

  it("returns null when airAt === now", () => {
    expect(airCountdown(now, now)).toBeNull();
  });

  it("returns null when airAt < now", () => {
    expect(airCountdown(now - 100, now)).toBeNull();
  });
});
