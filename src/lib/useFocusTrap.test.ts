import { describe, expect, it } from "vitest";
import { nextFocusIndex } from "./useFocusTrap";

describe("nextFocusIndex", () => {
  it("wraps forward from the last item", () => expect(nextFocusIndex(2, 3, false)).toBe(0));
  it("wraps backward from the first item", () => expect(nextFocusIndex(0, 3, true)).toBe(2));
  it("lets the browser move inside the container", () => {
    expect(nextFocusIndex(1, 3, false)).toBeNull();
    expect(nextFocusIndex(1, 3, true)).toBeNull();
  });
  it("pulls focus in when it is outside", () => {
    expect(nextFocusIndex(-1, 3, false)).toBe(0);
    expect(nextFocusIndex(-1, 3, true)).toBe(2);
  });
  it("does nothing without focusables", () => expect(nextFocusIndex(-1, 0, false)).toBeNull());
  it("single item stays put", () => expect(nextFocusIndex(0, 1, false)).toBe(0));
});
