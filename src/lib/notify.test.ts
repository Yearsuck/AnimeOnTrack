import { describe, expect, it } from "vitest";
import { NOTIFY_KEY, parseNotifyPref, shouldNotify } from "./notify";

describe("shouldNotify", () => {
  it("returns false when newCount is 0", () => {
    expect(shouldNotify({ newCount: 0, enabled: true, windowFocused: false })).toBe(false);
  });

  it("returns true when count is positive, notifications are enabled, and window is not focused", () => {
    expect(shouldNotify({ newCount: 3, enabled: true, windowFocused: false })).toBe(true);
  });

  it("returns false when disabled even if unfocused and count is positive", () => {
    expect(shouldNotify({ newCount: 3, enabled: false, windowFocused: false })).toBe(false);
  });

  it("returns false when window is focused even if enabled and count is positive", () => {
    expect(shouldNotify({ newCount: 3, enabled: true, windowFocused: true })).toBe(false);
  });

  it("returns false when disabled and window is focused", () => {
    expect(shouldNotify({ newCount: 3, enabled: false, windowFocused: true })).toBe(false);
  });

  it("returns false when count is negative", () => {
    expect(shouldNotify({ newCount: -1, enabled: true, windowFocused: false })).toBe(false);
  });
});

describe("parseNotifyPref", () => {
  it("defaults to true when value is null", () => {
    expect(parseNotifyPref(null)).toBe(true);
  });

  it("returns false for '0'", () => {
    expect(parseNotifyPref("0")).toBe(false);
  });

  it("returns false for 'false'", () => {
    expect(parseNotifyPref("false")).toBe(false);
  });

  it("returns true for '1'", () => {
    expect(parseNotifyPref("1")).toBe(true);
  });

  it("returns true for 'garbage'", () => {
    expect(parseNotifyPref("garbage")).toBe(true);
  });

  it("returns true for 'true'", () => {
    expect(parseNotifyPref("true")).toBe(true);
  });

  it("returns true for empty string", () => {
    expect(parseNotifyPref("")).toBe(true);
  });
});

describe("NOTIFY_KEY", () => {
  it("uses the expected localStorage key", () => {
    expect(NOTIFY_KEY).toBe("aot.notifyNewEpisodes");
  });
});
