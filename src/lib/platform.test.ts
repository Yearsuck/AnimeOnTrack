import { describe, it, expect, afterEach } from "vitest";
import { detectMobile } from "./platform";

describe("detectMobile", () => {
  const originalUserAgent = navigator.userAgent;

  afterEach(() => {
    Object.defineProperty(navigator, "userAgent", {
      value: originalUserAgent,
      configurable: true,
      writable: true,
    });
  });

  it("detects Android in custom userAgent parameter", () => {
    const androidUA =
      "Mozilla/5.0 (Linux; Android 16; Pixel 9 Pro) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Mobile Safari/537.36";
    expect(detectMobile(androidUA)).toBe(true);
  });

  it("detects lowercase android", () => {
    expect(detectMobile("mozilla/5.0 (linux; android 14)")).toBe(true);
  });

  it("returns false for Windows userAgent", () => {
    const windowsUA =
      "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36";
    expect(detectMobile(windowsUA)).toBe(false);
  });

  it("returns false for macOS userAgent", () => {
    const macUA =
      "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Safari/605.1.15";
    expect(detectMobile(macUA)).toBe(false);
  });

  it("returns false for Linux desktop userAgent", () => {
    const linuxUA =
      "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36";
    expect(detectMobile(linuxUA)).toBe(false);
  });

  it("falls back to navigator.userAgent when no parameter is passed", () => {
    Object.defineProperty(navigator, "userAgent", {
      value: "Mozilla/5.0 (Linux; Android 15; SM-S928B)",
      configurable: true,
      writable: true,
    });
    expect(detectMobile()).toBe(true);

    Object.defineProperty(navigator, "userAgent", {
      value: "Mozilla/5.0 (Windows NT 10.0; Win64; x64)",
      configurable: true,
      writable: true,
    });
    expect(detectMobile()).toBe(false);
  });
});
