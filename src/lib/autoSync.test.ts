import { describe, expect, it, vi } from "vitest";
import { createAutoSync } from "./autoSync";

describe("createAutoSync", () => {
  it("first trigger runs", async () => {
    let now = 1_000_000;
    const run = vi.fn().mockResolvedValue("ok");
    const autoSync = createAutoSync({
      run,
      minIntervalMs: 120_000,
      now: () => now,
    });

    await autoSync.trigger();
    expect(run).toHaveBeenCalledTimes(1);
  });

  it("second trigger within the interval does not run", async () => {
    let now = 1_000_000;
    const run = vi.fn().mockResolvedValue("ok");
    const autoSync = createAutoSync({
      run,
      minIntervalMs: 120_000,
      now: () => now,
    });

    await autoSync.trigger();
    expect(run).toHaveBeenCalledTimes(1);

    now += 60_000; // 60s later (< 120s)
    await autoSync.trigger();
    expect(run).toHaveBeenCalledTimes(1);
  });

  it("after the interval it runs again", async () => {
    let now = 1_000_000;
    const run = vi.fn().mockResolvedValue("ok");
    const autoSync = createAutoSync({
      run,
      minIntervalMs: 120_000,
      now: () => now,
    });

    await autoSync.trigger();
    expect(run).toHaveBeenCalledTimes(1);

    now += 120_000; // exactly at interval
    await autoSync.trigger();
    expect(run).toHaveBeenCalledTimes(2);

    now += 150_000; // beyond interval
    await autoSync.trigger();
    expect(run).toHaveBeenCalledTimes(3);
  });

  it("overlapping triggers while the first is pending run once", async () => {
    let now = 1_000_000;
    let resolveRun!: (value?: unknown) => void;
    const pendingPromise = new Promise((resolve) => {
      resolveRun = resolve;
    });
    const run = vi.fn().mockImplementation(() => pendingPromise);

    const autoSync = createAutoSync({
      run,
      minIntervalMs: 120_000,
      now: () => now,
    });

    const trigger1 = autoSync.trigger();
    expect(run).toHaveBeenCalledTimes(1);

    // Call trigger again while trigger1 is still pending (even if now advanced)
    now += 200_000;
    const trigger2 = autoSync.trigger();
    expect(run).toHaveBeenCalledTimes(1);

    resolveRun();
    await Promise.all([trigger1, trigger2]);

    expect(run).toHaveBeenCalledTimes(1);
  });

  it("a rejected run does not throw out of trigger and still counts as the last start", async () => {
    let now = 1_000_000;
    const run = vi.fn().mockRejectedValue(new Error("Network error"));

    const autoSync = createAutoSync({
      run,
      minIntervalMs: 120_000,
      now: () => now,
    });

    // Swallows rejection without throwing
    await expect(autoSync.trigger()).resolves.toBeUndefined();
    expect(run).toHaveBeenCalledTimes(1);

    // Within interval: does not run
    now += 50_000;
    await autoSync.trigger();
    expect(run).toHaveBeenCalledTimes(1);

    // After interval: runs again
    now += 120_000;
    run.mockResolvedValueOnce("recovered");
    await expect(autoSync.trigger()).resolves.toBeUndefined();
    expect(run).toHaveBeenCalledTimes(2);
  });
});
