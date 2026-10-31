import { describe, it, expect } from "vitest";
import { formatDecimal } from "./formatNumber";

describe("formatDecimal", () => {
  it("uses decimal comma in es/ca and dot in en", () => {
    expect(formatDecimal("es", 3.5)).toBe("3,5");
    expect(formatDecimal("ca", 3.5)).toBe("3,5");
    expect(formatDecimal("en", 3.5)).toBe("3.5");
    expect(formatDecimal("en", 3)).toBe("3.0");
  });
  it("non-finite -> dash", () => {
    expect(formatDecimal("es", NaN)).toBe("—");
  });
});
