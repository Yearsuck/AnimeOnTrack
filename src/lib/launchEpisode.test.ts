import { afterEach, describe, expect, it, vi } from "vitest";

const openEpisode = vi.fn();
vi.mock("../api", () => ({ openEpisode: (...a: unknown[]) => openEpisode(...a) }));

import { dispatchLaunch, getLaunchState, launchEpisode } from "./launchEpisode";
import { ERROR_SHOW_MS } from "./launchState";

afterEach(() => {
  vi.useRealTimers();
  openEpisode.mockReset();
  const s = getLaunchState();
  dispatchLaunch({ type: "dismiss" });
  dispatchLaunch({ type: "exited", seq: s.seq });
});

describe("launchEpisode", () => {
  it("a stale failure does not arm a timer that closes a newer curtain", async () => {
    vi.useFakeTimers();
    let rejectA!: (e: Error) => void;
    openEpisode.mockImplementationOnce(() => new Promise((_, rej) => (rejectA = rej)));
    const a = launchEpisode({ url: "a", title: "A" });
    openEpisode.mockImplementationOnce(() => new Promise(() => {}));
    void launchEpisode({ url: "b", title: "B" });
    rejectA(new Error("x"));
    await a;
    vi.advanceTimersByTime(ERROR_SHOW_MS + 10);
    expect(getLaunchState().phase).toBe("opening");
    expect(getLaunchState().req?.title).toBe("B");
  });

  it("a real failure shows the error then dismisses", async () => {
    vi.useFakeTimers();
    openEpisode.mockRejectedValueOnce(new Error("boom"));
    await launchEpisode({ url: "a", title: "A" });
    expect(getLaunchState().phase).toBe("error");
    vi.advanceTimersByTime(ERROR_SHOW_MS + 10);
    expect(getLaunchState().phase).toBe("closing");
  });
});
