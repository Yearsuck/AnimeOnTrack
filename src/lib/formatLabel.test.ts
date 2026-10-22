import { describe, expect, it } from "vitest";
import { es } from "../i18n/catalog/es";
import type { MessageKey } from "../i18n";
import { formatLabel, formatLabelKey, sortFormats } from "./formatLabel";

const t = (key: MessageKey) => es[key];

describe("formatLabel", () => {
  it("translates every AniList format code", () => {
    expect(formatLabel(t, "TV")).toBe("TV");
    expect(formatLabel(t, "MOVIE")).toBe("Película");
    expect(formatLabel(t, "SPECIAL")).toBe("Especial");
    expect(formatLabel(t, "TV_SHORT")).toBe("Corto");
    expect(formatLabel(t, "MUSIC")).toBe("Musical");
    expect(formatLabel(t, "OVA")).toBe("OVA");
    expect(formatLabel(t, "ONA")).toBe("ONA");
  });

  it("accepts the scraped sites' spelling and any case", () => {
    expect(formatLabelKey("Pelicula")).toBe("format.MOVIE");
    expect(formatLabelKey("movie")).toBe("format.MOVIE");
  });

  it("shows an unknown code as it came and nothing for an empty one", () => {
    expect(formatLabel(t, "Serie")).toBe("Serie");
    expect(formatLabel(t, null)).toBe("");
    expect(formatLabelKey(undefined)).toBeNull();
  });

  it("orders formats like AniList and puts unknown ones last", () => {
    expect(sortFormats(["ONA", "Serie", "MOVIE", "TV", "TV"])).toEqual(["TV", "MOVIE", "ONA", "Serie"]);
  });
});
