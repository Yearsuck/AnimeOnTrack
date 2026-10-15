import { describe, it, expect } from "vitest";
import { shouldShowSplash } from "./splash";

describe("shouldShowSplash", () => {
  it("shows splash when reduced motion is disabled and splash was not already shown", () => {
    expect(shouldShowSplash(false, false)).toBe(true);
  });

  it("suppresses splash when reduced motion is enabled", () => {
    expect(shouldShowSplash(true, false)).toBe(false);
  });

  it("suppresses splash when it has already been shown", () => {
    expect(shouldShowSplash(false, true)).toBe(false);
  });

  it("suppresses splash when reduced motion is enabled and it was already shown", () => {
    expect(shouldShowSplash(true, true)).toBe(false);
  });
});
