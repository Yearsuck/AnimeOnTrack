import { describe, expect, it } from "vitest";
import {
  startSeriesSequentially,
  summarizeStart,
  type StartResult,
} from "./startWatchingQueue";

describe("startWatchingQueue", () => {
  it("counts results accurately and summarizeStart rolls them up", async () => {
    const items = ["a", "b", "c"];
    const results: Record<string, StartResult> = {
      a: "started",
      b: "notFound",
      c: "alreadyStarted",
    };

    const counts = await startSeriesSequentially(items, async (item) => results[item]);
    expect(counts).toEqual({
      started: 1,
      alreadyStarted: 1,
      notFound: 1,
      failed: 0,
    });

    const summary = summarizeStart(counts);
    expect(summary).toEqual({
      started: 2,
      problems: 1,
    });
  });

  it("processes items strictly sequentially in input order without overlap", async () => {
    const items = [1, 2, 3, 4];
    let inFlight = 0;
    let maxInFlight = 0;
    const processedOrder: number[] = [];

    await startSeriesSequentially(items, async (item) => {
      inFlight++;
      maxInFlight = Math.max(maxInFlight, inFlight);
      expect(inFlight).toBe(1);

      // Yield event loop to test that concurrent tasks are not launched
      await new Promise((resolve) => setTimeout(resolve, 5));
      processedOrder.push(item);

      inFlight--;
      return "started";
    });

    expect(maxInFlight).toBe(1);
    expect(inFlight).toBe(0);
    expect(processedOrder).toEqual([1, 2, 3, 4]);
  });

  it("recovers from rejected start calls and continues remaining items as failed", async () => {
    const items = ["first", "error", "third"];

    const counts = await startSeriesSequentially(items, async (item) => {
      if (item === "error") {
        throw new Error("network / scraping error");
      }
      return "started";
    });

    expect(counts).toEqual({
      started: 2,
      alreadyStarted: 0,
      notFound: 0,
      failed: 1,
    });
    expect(summarizeStart(counts)).toEqual({
      started: 2,
      problems: 1,
    });
  });

  it("notifies onProgress for every processed item with (done, total, result)", async () => {
    const items = ["x", "y", "z"];
    const progressCalls: [number, number, StartResult][] = [];

    await startSeriesSequentially(
      items,
      async (item) => (item === "y" ? "notFound" : "started"),
      (done, total, result) => {
        progressCalls.push([done, total, result]);
      }
    );

    expect(progressCalls).toEqual([
      [1, 3, "started"],
      [2, 3, "notFound"],
      [3, 3, "started"],
    ]);
  });

  it("handles empty list without invoking start or onProgress", async () => {
    let startCalled = false;
    let progressCalled = false;

    const counts = await startSeriesSequentially(
      [],
      async () => {
        startCalled = true;
        return "started";
      },
      () => {
        progressCalled = true;
      }
    );

    expect(counts).toEqual({
      started: 0,
      alreadyStarted: 0,
      notFound: 0,
      failed: 0,
    });
    expect(summarizeStart(counts)).toEqual({
      started: 0,
      problems: 0,
    });
    expect(startCalled).toBe(false);
    expect(progressCalled).toBe(false);
  });
});
