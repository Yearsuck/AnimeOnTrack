import { describe, expect, it } from "vitest";
import { canMorph, findPoster, isFreshInteraction, rectVisible, type PosterNode } from "./viewTransition";

type N = PosterNode & { kids: N[] };

function node(tags: string[], kids: N[] = []): N {
  const n: N = {
    kids,
    parentElement: null,
    matches: (sel) => sel.split(",").some((s) => tags.includes(s.trim())),
    querySelector: (sel) => {
      if (sel === ".poster img") return n.querySelector(".poster")?.querySelector("img") ?? null;
      const find = (list: N[]): N | null => {
        for (const c of list) {
          if (c.matches(sel)) return c;
          const deeper = find(c.kids);
          if (deeper) return deeper;
        }
        return null;
      };
      return find(n.kids);
    },
  };
  kids.forEach((c) => (c.parentElement = n));
  return n;
}

describe("canMorph", () => {
  it("needs support and no reduced motion", () => {
    expect(canMorph({ supported: true, reducedMotion: false })).toBe(true);
    expect(canMorph({ supported: false, reducedMotion: false })).toBe(false);
    expect(canMorph({ supported: true, reducedMotion: true })).toBe(false);
  });
});

describe("rectVisible", () => {
  const r = (left: number, top: number, w: number, h: number) => ({
    left,
    top,
    right: left + w,
    bottom: top + h,
    width: w,
    height: h,
  });
  it("accepts partially visible boxes", () => {
    expect(rectVisible(r(10, 10, 100, 150), 800, 600)).toBe(true);
    expect(rectVisible(r(-50, 500, 100, 150), 800, 600)).toBe(true);
  });
  it("rejects off-screen or empty boxes", () => {
    expect(rectVisible(r(10, 700, 100, 150), 800, 600)).toBe(false);
    expect(rectVisible(r(-200, 10, 100, 150), 800, 600)).toBe(false);
    expect(rectVisible(r(10, 10, 0, 150), 800, 600)).toBe(false);
  });
});

describe("isFreshInteraction", () => {
  it("accepts recent, rejects stale or future", () => {
    expect(isFreshInteraction(1000, 500)).toBe(true);
    expect(isFreshInteraction(5000, 500)).toBe(false);
    expect(isFreshInteraction(100, 500)).toBe(false);
  });
});

describe("findPoster", () => {
  it("prefers the poster img inside the clicked card", () => {
    const img = node(["img"]);
    const title = node([".card-title"]);
    const card = node([".card"], [node([".poster"], [img]), title]);
    node([".view-panel"], [card]);
    expect(findPoster(title)).toBe(img);
  });
  it("returns the target itself when it is an img", () => {
    const img = node(["img"]);
    node([".card"], [img]);
    expect(findPoster(img)).toBe(img);
  });
  it("falls back to the .poster box when it has no img", () => {
    const poster = node([".poster"]);
    const btn = node(["button"]);
    node([".card"], [poster, btn]);
    expect(findPoster(btn)).toBe(poster);
  });
  it("stops at the tab panel", () => {
    const chart = node(["canvas"]);
    node([".view-panel"], [chart, node(["img"])]);
    expect(findPoster(chart)).toBeNull();
  });
  it("is null for no target", () => {
    expect(findPoster(null)).toBeNull();
  });
});
