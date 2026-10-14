import { describe, expect, it } from "vitest";
import { downloadPercent, parseUpdatePref } from "./updater";

describe("parseUpdatePref", () => {
  it("is on by default and for any stored value but off", () => {
    expect(parseUpdatePref(null)).toBe(true);
    expect(parseUpdatePref("1")).toBe(true);
    expect(parseUpdatePref("")).toBe(true);
  });
  it("is off only for 0/false", () => {
    expect(parseUpdatePref("0")).toBe(false);
    expect(parseUpdatePref("false")).toBe(false);
  });
});

describe("downloadPercent", () => {
  it("is null while the total is unknown", () => {
    expect(downloadPercent(10, undefined)).toBeNull();
    expect(downloadPercent(10, 0)).toBeNull();
  });
  it("floors and clamps to 100", () => {
    expect(downloadPercent(1, 3)).toBe(33);
    expect(downloadPercent(5, 4)).toBe(100);
  });
});
