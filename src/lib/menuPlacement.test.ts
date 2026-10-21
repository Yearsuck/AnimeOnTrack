import { describe, expect, it } from "vitest";
import { computeMenuPosition, nextMenuIndex } from "./menuPlacement";

const viewport = { width: 1000, height: 800 };
const menu = { width: 170, height: 100 };

describe("computeMenuPosition", () => {
  it("opens below, right-aligned by default", () => {
    const r = computeMenuPosition({ top: 100, left: 500, right: 532, bottom: 132 }, menu, viewport);
    expect(r.placement).toBe("bottom");
    expect(r.top).toBe(136);
    expect(r.left).toBe(532 - 170);
  });
  it("flips up near the bottom", () => {
    const r = computeMenuPosition({ top: 740, left: 500, right: 532, bottom: 772 }, menu, viewport);
    expect(r.placement).toBe("top");
    expect(r.top).toBe(740 - 4 - 100);
  });
  it("falls back to left-aligned then clamps near the left edge", () => {
    const r = computeMenuPosition({ top: 100, left: 10, right: 42, bottom: 132 }, menu, viewport);
    expect(r.left).toBe(10);
    const r2 = computeMenuPosition({ top: 100, left: 2, right: 20, bottom: 132 }, menu, viewport);
    expect(r2.left).toBe(8);
  });
  it("clamps to the right edge", () => {
    const r = computeMenuPosition({ top: 100, left: 980, right: 1000, bottom: 132 }, menu, viewport);
    expect(r.left + menu.width).toBeLessThanOrEqual(992);
  });
});

describe("nextMenuIndex", () => {
  it("wraps with arrows", () => {
    expect(nextMenuIndex("ArrowDown", 2, 3)).toBe(0);
    expect(nextMenuIndex("ArrowUp", 0, 3)).toBe(2);
    expect(nextMenuIndex("ArrowDown", -1, 3)).toBe(0);
  });
  it("handles Home/End and ignores others", () => {
    expect(nextMenuIndex("Home", 2, 3)).toBe(0);
    expect(nextMenuIndex("End", 0, 3)).toBe(2);
    expect(nextMenuIndex("a", 0, 3)).toBeNull();
    expect(nextMenuIndex("ArrowDown", 0, 0)).toBeNull();
  });
});
