import { describe, it, expect } from "vitest";
import {
  clampRatio,
  nextPresence,
  scanMode,
  isProgressDone,
  formatProgressLabel,
  EXIT_MS,
  DONE_HOLD_MS,
  type RefreshProgressPayload,
} from "./progress";

describe("clampRatio", () => {
  it("divides value by max", () => {
    expect(clampRatio(3, 12)).toBe(0.25);
  });
  it("clamps to 0..1", () => {
    expect(clampRatio(-4, 10)).toBe(0);
    expect(clampRatio(15, 10)).toBe(1);
  });
  it("is 0 for a missing, zero, negative or non-finite max", () => {
    expect(clampRatio(5, 0)).toBe(0);
    expect(clampRatio(5, -1)).toBe(0);
    expect(clampRatio(5, Number.NaN)).toBe(0);
    expect(clampRatio(5, Number.POSITIVE_INFINITY)).toBe(0);
  });
  it("is 0 for a missing or NaN value", () => {
    expect(clampRatio(undefined, 10)).toBe(0);
    expect(clampRatio(Number.NaN, 10)).toBe(0);
  });
});

describe("scanMode", () => {
  it("is indeterminate before the first series finishes", () => {
    expect(scanMode(0, 12, null)).toBe("indeterminate");
    expect(scanMode(0, 12, "extracting")).toBe("indeterminate");
  });
  it("is indeterminate while a page is opening or behind Cloudflare", () => {
    expect(scanMode(4, 12, "opening")).toBe("indeterminate");
    expect(scanMode(4, 12, "verifying")).toBe("indeterminate");
  });
  it("is determinate while extracting or downloading covers", () => {
    expect(scanMode(4, 12, "extracting")).toBe("determinate");
    expect(scanMode(4, 12, "covers")).toBe("determinate");
    expect(scanMode(4, 12, null)).toBe("determinate");
  });
  it("is determinate once complete, whatever the stale stage says", () => {
    expect(scanMode(12, 12, "verifying")).toBe("determinate");
    expect(scanMode(13, 12, null)).toBe("determinate");
  });
  it("is determinate when total is 0 to avoid an indeterminate sweep", () => {
    expect(scanMode(0, 0, null)).toBe("determinate");
    expect(scanMode(0, 0, "opening")).toBe("determinate");
  });
});

describe("progress done and label resolution", () => {
  it("shows title and does not mark done for {current:1, total:1, done:false, title:'Listado completo'}", () => {
    const payload: RefreshProgressPayload = {
      current: 1,
      total: 1,
      done: false,
      title: "Listado completo",
    };
    expect(isProgressDone(payload)).toBe(false);
    expect(formatProgressLabel(payload, null, {}, "Completado")).toBe("Listado completo");
  });

  it("shows title and does not mark done for {current:N, total:N, done:false, title:'Descargando carátulas: X'}", () => {
    const payload: RefreshProgressPayload = {
      current: 10,
      total: 10,
      done: false,
      title: "Descargando carátulas: X",
    };
    expect(isProgressDone(payload)).toBe(false);
    expect(formatProgressLabel(payload, null, {}, "Completado")).toBe("Descargando carátulas: X");
  });

  it("shows Completado and marks done for {done:true}", () => {
    const payload: RefreshProgressPayload = {
      current: 10,
      total: 10,
      done: true,
      title: "Completado",
    };
    expect(isProgressDone(payload)).toBe(true);
    expect(formatProgressLabel(payload, null, {}, "Completado")).toBe("Completado");
  });

  it("appends stage when not done", () => {
    const payload: RefreshProgressPayload = {
      current: 2,
      total: 10,
      done: false,
      title: "Series A",
    };
    expect(
      formatProgressLabel(payload, "extracting", { extracting: "Extrayendo" }, "Completado")
    ).toBe("Series A — Extrayendo");
  });

  it("shows title and is determinate 0 when total is 0 and done is false (no Completado 0/0 sweep)", () => {
    const payload: RefreshProgressPayload = {
      current: 0,
      total: 0,
      done: false,
      title: "Preparando...",
    };
    expect(isProgressDone(payload)).toBe(false);
    expect(formatProgressLabel(payload, null, {}, "Completado")).toBe("Preparando...");
    expect(scanMode(payload.current, payload.total, null)).toBe("determinate");
  });
});

describe("nextPresence", () => {
  it("shows when asked, from any state (a re-show interrupts a leave)", () => {
    expect(nextPresence("hidden", true)).toBe("shown");
    expect(nextPresence("shown", true)).toBe("shown");
    expect(nextPresence("leaving", true)).toBe("shown");
  });
  it("starts leaving from shown instead of unmounting", () => {
    expect(nextPresence("shown", false)).toBe("leaving");
    expect(nextPresence("leaving", false)).toBe("leaving");
  });
  it("stays hidden when it was never shown", () => {
    expect(nextPresence("hidden", false)).toBe("hidden");
  });
});

describe("timings", () => {
  it("unmounts after the leave transition (--dur-fast, 120ms) has finished", () => {
    expect(EXIT_MS).toBeGreaterThanOrEqual(120);
    expect(EXIT_MS).toBeLessThanOrEqual(300);
  });
  it("holds the completed state longer than the fill's --dur-slow (320ms) run to 100%", () => {
    expect(DONE_HOLD_MS).toBeGreaterThan(320);
  });
});
