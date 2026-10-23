import { describe, expect, it } from "vitest";
import {
  INITIAL_LAUNCH_STATE as S0,
  MIN_SHOW_MS,
  holdRemaining,
  launchReducer,
  type LaunchRequest,
} from "./launchState";

const req: LaunchRequest = { title: "A", episode: "3", cover: null, site: null, origin: null };

describe("launchReducer", () => {
  it("opens, then closes when the window resolves", () => {
    const a = launchReducer(S0, { type: "start", req });
    expect(a.phase).toBe("opening");
    const b = launchReducer(a, { type: "resolved", seq: a.seq });
    expect(b.phase).toBe("closing");
    const c = launchReducer(b, { type: "exited", seq: a.seq });
    expect(c.phase).toBe("idle");
    expect(c.req).toBeNull();
  });

  it("shows the error in place and then closes", () => {
    const a = launchReducer(S0, { type: "start", req });
    const b = launchReducer(a, { type: "failed", seq: a.seq, message: "boom" });
    expect(b).toMatchObject({ phase: "error", error: "boom" });
    expect(launchReducer(b, { type: "dismiss" }).phase).toBe("closing");
  });

  it("early dismissal wins over a later resolve or failure", () => {
    const a = launchReducer(S0, { type: "start", req });
    const d = launchReducer(a, { type: "dismiss" });
    expect(launchReducer(d, { type: "failed", seq: a.seq, message: "x" })).toBe(d);
    expect(launchReducer(d, { type: "resolved", seq: a.seq })).toBe(d);
  });

  it("ignores events from a superseded launch", () => {
    const a = launchReducer(S0, { type: "start", req });
    const b = launchReducer(a, { type: "start", req });
    expect(launchReducer(b, { type: "resolved", seq: a.seq })).toBe(b);
    expect(launchReducer(b, { type: "exited", seq: a.seq })).toBe(b);
  });

  it("dismiss while idle is a no-op", () => {
    expect(launchReducer(S0, { type: "dismiss" })).toBe(S0);
  });
});

describe("holdRemaining", () => {
  it("tops up to the minimum and never goes negative", () => {
    expect(holdRemaining(100)).toBe(MIN_SHOW_MS - 100);
    expect(holdRemaining(MIN_SHOW_MS)).toBe(0);
    expect(holdRemaining(5000)).toBe(0);
    expect(holdRemaining(-20)).toBe(MIN_SHOW_MS);
  });
});
