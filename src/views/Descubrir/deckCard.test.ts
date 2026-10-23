import { describe, expect, it } from "vitest";
import type { CatalogAnime, SwipeCard } from "../../types";
import { deckCardInfo, teaserText } from "./deckCard";

// Echoes the key and params so assertions read the structure, not a language.
const t = ((key: string, p?: Record<string, string | number>) =>
  p ? `${key}(${Object.values(p).join(",")})` : key) as never;

function card(catalog: Partial<CatalogAnime> | null, extra: Partial<SwipeCard> = {}): SwipeCard {
  return {
    title: "Frieren",
    url: "https://anilist.co/anime/154587",
    poster_url: null,
    kind: "TV",
    matched_genre: "Fantasy",
    catalog: catalog
      ? {
          id: 154587,
          title: "Frieren",
          cover_url: null,
          format: "TV",
          genres: ["Adventure", "Drama", "Fantasy"],
          episodes: 28,
          average_score: 91,
          popularity: 1,
          url: "https://anilist.co/anime/154587",
          studio: "Madhouse",
          start_date: Date.UTC(2023, 8, 29) / 1000,
          ...catalog,
        }
      : null,
    ...extra,
  };
}

describe("deckCardInfo", () => {
  it("builds facts, genres and score from the catalog row", () => {
    const info = deckCardInfo(card({ title_romaji: "Sousou no Frieren" }), t);
    expect(info.facts).toBe("format.TV · 2023 · catalog.episodesCount(28) · Madhouse");
    expect(info.genres).toEqual(["Adventure", "Drama", "Fantasy"]);
    expect(info.score).toBe("91%");
    expect(info.alt).toBe("Sousou no Frieren");
  });

  it("says upcoming instead of a year and hides an alt title equal to the shown one", () => {
    const info = deckCardInfo(card({ status: "NOT_YET_RELEASED", title_english: "frieren", episodes: null }), t);
    expect(info.facts).toBe("format.TV · filters.statusNotYetReleased · Madhouse");
    expect(info.alt).toBeNull();
  });

  it("falls back to the card alone when the catalog row is missing", () => {
    const info = deckCardInfo(card(null), t);
    expect(info.facts).toBe("format.TV");
    expect(info.genres).toEqual(["Fantasy"]);
    expect(info.score).toBeNull();
  });
});

describe("teaserText", () => {
  it("joins paragraphs and leaves spoilers out", () => {
    expect(
      teaserText([
        [{ text: "One.", spoiler: false }],
        [
          { text: "Two", spoiler: false },
          { text: "secret", spoiler: true },
        ],
        [{ text: "hidden", spoiler: true }],
      ])
    ).toBe("One. Two");
  });
});
