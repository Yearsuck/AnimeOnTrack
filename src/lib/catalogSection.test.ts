import { describe, expect, it } from "vitest";
import { CATALOG_SECTIONS, discoverSubViewFor, isCatalogSection } from "./catalogSection";

describe("catalogSection", () => {
  it("recognises exactly the three sections", () => {
    for (const s of CATALOG_SECTIONS) expect(isCatalogSection(s)).toBe(true);
    expect(isCatalogSection("descubrir")).toBe(false);
    expect(isCatalogSection(undefined)).toBe(false);
  });

  it("maps sections to the Descubrir sub-view", () => {
    expect(discoverSubViewFor("swipe")).toBe("swipe");
    expect(discoverSubViewFor("listas")).toBe("listas");
    expect(discoverSubViewFor("explore")).toBe("swipe");
  });
});
