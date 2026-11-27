import { describe, it, expect } from "vitest";
import { applyPatch, rollbackPatch } from "./settingSave";

type S = { a: boolean; b: string };
const base: S = { a: true, b: "x" };

describe("settingSave", () => {
  it("applyPatch sets the optimistic value without mutating", () => {
    const next = applyPatch(base, { a: false });
    expect(next).toEqual({ a: false, b: "x" });
    expect(base.a).toBe(true);
  });

  it("rollbackPatch restores the patched key", () => {
    const optimistic = applyPatch(base, { a: false });
    expect(rollbackPatch(optimistic, base, { a: false })).toEqual(base);
  });

  it("rollbackPatch keeps keys changed by another save", () => {
    const optimistic = applyPatch(applyPatch(base, { a: false }), { b: "y" });
    expect(rollbackPatch(optimistic, base, { a: false })).toEqual({ a: true, b: "y" });
  });
});
