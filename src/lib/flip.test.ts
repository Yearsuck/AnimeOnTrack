import { describe, expect, it } from "vitest";
import { flipDelta, flipTransform } from "./flip";

describe("flipDelta", () => {
  const target = { left: 100, top: 50, width: 700, height: 600 };

  it("centres the target on the origin", () => {
    const d = flipDelta({ left: 0, top: 0, width: 140, height: 210 }, target)!;
    expect(d.dx).toBeCloseTo(70 - 450);
    expect(d.dy).toBeCloseTo(105 - 350);
  });

  it("uses the larger ratio so the origin is covered", () => {
    const d = flipDelta({ left: 0, top: 0, width: 140, height: 210 }, target)!;
    expect(d.scale).toBeCloseTo(0.35);
  });

  it("clamps to 1 when the origin is bigger than the target", () => {
    expect(flipDelta({ left: 0, top: 0, width: 2000, height: 2000 }, target)!.scale).toBe(1);
  });

  it("clamps tiny origins to a visible minimum", () => {
    expect(flipDelta({ left: 0, top: 0, width: 2, height: 2 }, target)!.scale).toBe(0.12);
  });

  it("returns null for empty boxes", () => {
    expect(flipDelta({ left: 0, top: 0, width: 0, height: 10 }, target)).toBeNull();
    expect(flipDelta({ left: 0, top: 0, width: 10, height: 10 }, { ...target, width: 0 })).toBeNull();
  });

  it("formats a transform string", () => {
    expect(flipTransform({ dx: -10.12345, dy: 5, scale: 0.5 })).toBe("translate(-10.123px, 5px) scale(0.5)");
  });
});
