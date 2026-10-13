import { describe, expect, it } from "vitest";
import { isUnlinkedCatalogRow } from "./catalogLink";
import type { Series } from "../types";

function createSeries(slug: string): Series {
  return {
    id: 1,
    slug,
    title: "Test Series",
    url: "https://example.com/anime/test",
    cover_url: null,
    is_airing: false,
    followed: false,
    next_episode_at: null,
    site_episode_count: null,
  };
}

describe("isUnlinkedCatalogRow", () => {
  it("returns true for synthetic slug 'anilist-123'", () => {
    expect(isUnlinkedCatalogRow(createSeries("anilist-123"))).toBe(true);
  });

  it("returns false for regular site slug 'tensei-kizoku'", () => {
    expect(isUnlinkedCatalogRow(createSeries("tensei-kizoku"))).toBe(false);
  });

  it("returns false when slug does not start with 'anilist-', e.g. 'my-anilist-1'", () => {
    expect(isUnlinkedCatalogRow(createSeries("my-anilist-1"))).toBe(false);
  });
});
