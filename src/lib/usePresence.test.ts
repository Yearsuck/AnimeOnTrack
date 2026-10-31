import { describe, it, expect } from "vitest";
import { stepPresence, nextPresence, exitPresence, type Presence } from "./progress";

describe("usePresence transition logic", () => {
  it("transitions hidden -> show true to shown immediately", () => {
    expect(stepPresence("hidden", "show")).toBe("shown");
    expect(nextPresence("hidden", true)).toBe("shown");
  });

  it("transitions shown -> false to leaving", () => {
    expect(stepPresence("shown", "hide")).toBe("leaving");
    expect(nextPresence("shown", false)).toBe("leaving");
  });

  it("transitions leaving -> exit to hidden after exit", () => {
    expect(stepPresence("leaving", "exit")).toBe("hidden");
    expect(exitPresence("leaving")).toBe("hidden");
  });

  it("transitions leaving -> true to shown immediately (interrupting leave)", () => {
    expect(stepPresence("leaving", "show")).toBe("shown");
    expect(nextPresence("leaving", true)).toBe("shown");
  });

  it("stays hidden when false is passed from hidden state", () => {
    expect(stepPresence("hidden", "hide")).toBe("hidden");
    expect(nextPresence("hidden", false)).toBe("hidden");
  });

  it("does not affect shown or hidden states on exit event", () => {
    expect(stepPresence("shown", "exit")).toBe("shown");
    expect(stepPresence("hidden", "exit")).toBe("hidden");
    expect(exitPresence("shown")).toBe("shown");
    expect(exitPresence("hidden")).toBe("hidden");
  });

  it("follows the complete lifecycle: hidden -> show -> hide -> exit", () => {
    let state: Presence = "hidden";
    // 1. Initial show -> immediately "shown"
    state = nextPresence(state, true);
    expect(state).toBe("shown");

    // 2. Hide -> transitions to "leaving"
    state = nextPresence(state, false);
    expect(state).toBe("leaving");

    // 3. Exit timeout completes -> transitions to "hidden"
    state = exitPresence(state);
    expect(state).toBe("hidden");
  });

  it("interrupts exit when re-shown during leaving", () => {
    let state: Presence = "shown";
    // Hide -> leaving
    state = nextPresence(state, false);
    expect(state).toBe("leaving");

    // Re-show arrives while still leaving -> immediately "shown"
    state = nextPresence(state, true);
    expect(state).toBe("shown");

    // A stale exit event does not hide a shown element
    state = exitPresence(state);
    expect(state).toBe("shown");
  });
});
