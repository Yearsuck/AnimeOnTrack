import { describe, expect, it, vi } from "vitest";
import { BackLevel, createBackStack, type HistoryLike } from "./backStack";

/** In-memory History with the browser's timing: pushState is synchronous,
 *  go()/back() traverse later and fire one popstate per traversal. */
function fakeHistory(initialState: unknown = null, goDelayMs = 0) {
  const entries: unknown[] = [initialState];
  let index = 0;
  const listeners = new Set<() => void>();
  const history: HistoryLike = {
    get state() {
      return entries[index];
    },
    pushState(state: unknown) {
      entries.splice(index + 1);
      entries.push(state);
      index++;
    },
    go(delta: number) {
      setTimeout(() => {
        const next = Math.max(0, Math.min(entries.length - 1, index + delta));
        if (next === index) return;
        index = next;
        listeners.forEach((l) => l());
      }, goDelayMs);
    },
  };
  const target = {
    addEventListener: (_: "popstate", l: () => void) => void listeners.add(l),
    removeEventListener: (_: "popstate", l: () => void) => void listeners.delete(l),
  };
  return {
    history,
    target,
    /** Entries at or below the current one: what Android's back walks through. */
    depth: () => index,
    /** The system back button (Android) / mouse back (desktop). */
    back: () => history.go(-1),
  };
}

const settle = () => new Promise((r) => setTimeout(r, 5));

describe("backStack", () => {
  it("pushes one history entry per open layer and none for an empty stack", async () => {
    const h = fakeHistory();
    const stack = createBackStack(h.history, h.target);
    expect(h.depth()).toBe(0);
    stack.push(() => {});
    stack.push(() => {});
    await settle();
    expect(h.depth()).toBe(2);
  });

  it("system back closes only the top layer, in LIFO order", async () => {
    const h = fakeHistory();
    const stack = createBackStack(h.history, h.target);
    const closed: string[] = [];
    stack.push(() => closed.push("view"));
    stack.push(() => closed.push("sheet"));
    stack.push(() => closed.push("menu"));
    await settle();

    h.back();
    await settle();
    expect(closed).toEqual(["menu"]);
    expect(h.depth()).toBe(2);

    h.back();
    await settle();
    expect(closed).toEqual(["menu", "sheet"]);

    h.back();
    await settle();
    expect(closed).toEqual(["menu", "sheet", "view"]);
    expect(h.depth()).toBe(0);
    expect(stack.size()).toBe(0);
  });

  it("closing from the UI consumes its own entry once and closes nothing else", async () => {
    const h = fakeHistory();
    const stack = createBackStack(h.history, h.target);
    const below = vi.fn();
    const top = vi.fn();
    stack.push(below);
    const release = stack.push(top);
    await settle();
    expect(h.depth()).toBe(2);

    release();
    release(); // double release (Escape + unmount) must not pop twice
    await settle();
    expect(h.depth()).toBe(1);
    expect(top).not.toHaveBeenCalled();
    expect(below).not.toHaveBeenCalled();
    expect(stack.size()).toBe(1);
  });

  it("the release that follows a back-triggered close does not pop again", async () => {
    const h = fakeHistory();
    const stack = createBackStack(h.history, h.target);
    const below = vi.fn();
    stack.push(below);
    let release: () => void = () => {};
    release = stack.push(() => release()); // close() unmounts the layer, which releases it
    await settle();

    h.back();
    await settle();
    expect(h.depth()).toBe(1);
    expect(below).not.toHaveBeenCalled();
    expect(stack.size()).toBe(1);
  });

  it("releasing a layer under the top keeps the top layer and the entry count right", async () => {
    const h = fakeHistory();
    const stack = createBackStack(h.history, h.target);
    const closed: string[] = [];
    const releaseView = stack.push(() => closed.push("view"));
    stack.push(() => closed.push("detail"));
    await settle();

    releaseView();
    await settle();
    expect(h.depth()).toBe(1);
    expect(closed).toEqual([]);

    h.back();
    await settle();
    expect(closed).toEqual(["detail"]);
    expect(h.depth()).toBe(0);
  });

  it("closing one layer and opening another in the same tick nets to no traversal", async () => {
    const h = fakeHistory();
    const stack = createBackStack(h.history, h.target);
    const go = vi.spyOn(h.history, "go");
    const release = stack.push(() => {});
    await settle();
    release();
    const second = vi.fn();
    stack.push(second);
    await settle();
    expect(go).not.toHaveBeenCalled();
    expect(h.depth()).toBe(1);

    h.back();
    await settle();
    expect(second).toHaveBeenCalledOnce();
    expect(h.depth()).toBe(0);
  });

  it("a layer opened while our own traversal is in flight is pushed after it lands", async () => {
    const h = fakeHistory();
    const stack = createBackStack(h.history, h.target);
    const a = stack.push(() => {});
    const b = stack.push(() => {});
    await settle();
    a();
    b();
    await Promise.resolve(); // the go(-2) is issued, its popstate not yet fired
    const c = vi.fn();
    stack.push(c);
    await settle();
    expect(h.depth()).toBe(1);

    h.back();
    await settle();
    expect(c).toHaveBeenCalledOnce();
    expect(h.depth()).toBe(0);
  });

  it("a popstate fired by our own traversal never closes a layer", async () => {
    const h = fakeHistory();
    const stack = createBackStack(h.history, h.target);
    const keep = vi.fn();
    stack.push(keep);
    const release = stack.push(() => {});
    await settle();
    release();
    await settle();
    expect(keep).not.toHaveBeenCalled();
    expect(stack.size()).toBe(1);
  });

  it("a multi-step back closes every layer above the landing entry", async () => {
    const h = fakeHistory();
    const stack = createBackStack(h.history, h.target);
    const closed: string[] = [];
    stack.push(() => closed.push("a"));
    stack.push(() => closed.push("b"));
    stack.push(() => closed.push("c"));
    await settle();
    h.history.go(-2);
    await settle();
    expect(closed).toEqual(["c", "b"]);
    expect(h.depth()).toBe(1);
  });

  it("main views: one entry while away from home, none after going back home", async () => {
    // App keeps one layer open while `view !== home`: switching between
    // non-home tabs keeps that single entry; back (or a click on home) drops it.
    const h = fakeHistory();
    const stack = createBackStack(h.history, h.target);
    let view = "pending";
    let releaseView: (() => void) | null = null;
    const navigate = (next: string) => {
      view = next;
      const away = next !== "pending";
      if (away && !releaseView) releaseView = stack.push(() => navigate("pending"));
      if (!away && releaseView) {
        releaseView();
        releaseView = null;
      }
    };
    navigate("airing");
    navigate("library");
    navigate("stats");
    await settle();
    expect(h.depth()).toBe(1);

    h.back();
    await settle();
    expect(view).toBe("pending");
    expect(h.depth()).toBe(0);

    navigate("settings");
    await settle();
    navigate("pending"); // tab click home: entry consumed, nothing else closed
    await settle();
    expect(h.depth()).toBe(0);
    expect(stack.size()).toBe(0);
  });

  it("overlays close before sub-tabs, and sub-tabs before the main view, whatever the open order", async () => {
    const h = fakeHistory();
    const stack = createBackStack(h.history, h.target);
    const closed: string[] = [];
    // Child effects run before the parent's: the drawer registers first.
    stack.push(() => closed.push("drawer"), BackLevel.overlay);
    stack.push(() => closed.push("subtab"), BackLevel.tab);
    stack.push(() => closed.push("view"), BackLevel.view);
    stack.push(() => closed.push("menu"));
    await settle();
    expect(h.depth()).toBe(4);
    for (let i = 0; i < 4; i++) {
      h.back();
      await settle();
    }
    expect(closed).toEqual(["menu", "drawer", "subtab", "view"]);
    expect(h.depth()).toBe(0);
  });

  it("a back press with no layer open leaves history alone (Android exits)", async () => {
    const h = fakeHistory();
    const stack = createBackStack(h.history, h.target);
    const release = stack.push(() => {});
    await settle();
    release();
    await settle();
    expect(h.depth()).toBe(0);
    expect(stack.size()).toBe(0);
  });

  it("after a reload, leftover entries from the previous page are dropped", async () => {
    const h = fakeHistory();
    // Simulate a previous page life that left two of our entries behind.
    h.history.pushState({ aotBack: 1 }, "");
    h.history.pushState({ aotBack: 2 }, "");
    createBackStack(h.history, h.target);
    await settle();
    expect(h.depth()).toBe(0);
  });

  it("a traversal slower than the timeout never triggers a second one", async () => {
    // A stalled main thread delays our own go(-1) past the timeout: the late
    // popstate must not be read as a user back that closes the view below.
    const h = fakeHistory(null, 40);
    const stack = createBackStack(h.history, h.target, { traversalTimeoutMs: 5 });
    const closed: string[] = [];
    stack.push(() => closed.push("view"), BackLevel.view);
    const releaseMenu = stack.push(() => closed.push("menu"));
    await settle();
    expect(h.depth()).toBe(2);

    releaseMenu();
    await new Promise((r) => setTimeout(r, 120));
    expect(closed).toEqual([]);
    expect(stack.size()).toBe(1);
    expect(h.depth()).toBe(1);
  });

  it("stops listening after dispose", async () => {
    const h = fakeHistory();
    const stack = createBackStack(h.history, h.target);
    const close = vi.fn();
    stack.push(close);
    await settle();
    stack.dispose();
    h.back();
    await settle();
    expect(close).not.toHaveBeenCalled();
  });
});
