import { describe, expect, it } from "vitest";
import { measureIndicator, tabDirection } from "./slidingIndicator";

describe("measureIndicator", () => {
  it("returns the item's offset and width", () => {
    expect(measureIndicator({ offsetLeft: 40, offsetWidth: 80 }, 300)).toEqual({ x: 40, w: 80 });
  });
  it("is null when the container or item is not laid out", () => {
    expect(measureIndicator({ offsetLeft: 0, offsetWidth: 80 }, 0)).toBeNull();
    expect(measureIndicator({ offsetLeft: 0, offsetWidth: 0 }, 300)).toBeNull();
  });
  it("clamps to the container", () => {
    expect(measureIndicator({ offsetLeft: -5, offsetWidth: 50 }, 300)).toEqual({ x: 0, w: 50 });
    expect(measureIndicator({ offsetLeft: 280, offsetWidth: 50 }, 300)).toEqual({ x: 280, w: 20 });
    expect(measureIndicator({ offsetLeft: 320, offsetWidth: 50 }, 300)).toBeNull();
  });
});

describe("tabDirection", () => {
  const order = ["a", "b", "c"] as const;
  it("is fwd going right and back going left", () => {
    expect(tabDirection(order, "a", "c")).toBe("fwd");
    expect(tabDirection(order, "c", "a")).toBe("back");
  });
  it("defaults to fwd for unknown views", () => {
    expect(tabDirection(order, "x" as never, "a")).toBe("fwd");
  });
});
