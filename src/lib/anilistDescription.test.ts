import { describe, expect, it } from "vitest";
import { hasSpoilers, initialSynopsisSource, parseDescription } from "./anilistDescription";
import { aniListSearchUrl, aniListTarget, isAniListUrl, seasonOf, sitePageUrl } from "./anilistLinks";

describe("parseDescription", () => {
  it("turns <br> runs into paragraphs and strips tags", () => {
    const out = parseDescription("First <i>line</i>.<br><br>Second &amp; last.<br>\n(Source: X)");
    expect(out).toEqual([
      [{ text: "First line.", spoiler: false }],
      [{ text: "Second & last.", spoiler: false }],
      [{ text: "(Source: X)", spoiler: false }],
    ]);
  });

  it("flags spoiler text, including spoilers that span paragraphs", () => {
    const out = parseDescription("Intro ~!he dies<br>and returns!~ end");
    expect(out).toEqual([
      [
        { text: "Intro ", spoiler: false },
        { text: "he dies", spoiler: true },
      ],
      [
        { text: "and returns", spoiler: true },
        { text: " end", spoiler: false },
      ],
    ]);
    expect(hasSpoilers(out)).toBe(true);
  });

  it("never yields markup: script tags and encoded tags end up as inert text", () => {
    const out = parseDescription("<script>alert(1)</script>Hi &lt;b&gt;x&lt;/b&gt; __bold__");
    expect(out).toEqual([[{ text: "alert(1)Hi <b>x</b> bold", spoiler: false }]]);
  });

  it("handles empty input", () => {
    expect(parseDescription(null)).toEqual([]);
    expect(parseDescription("  <br> ")).toEqual([]);
  });
});

describe("initialSynopsisSource", () => {
  it("prefers the Spanish site synopsis for es/ca and AniList for en", () => {
    expect(initialSynopsisSource("es", true, true)).toBe("site");
    expect(initialSynopsisSource("ca", true, true)).toBe("site");
    expect(initialSynopsisSource("en", true, true)).toBe("anilist");
  });
  it("falls back to whatever exists", () => {
    expect(initialSynopsisSource("es", false, true)).toBe("anilist");
    expect(initialSynopsisSource("en", true, false)).toBe("site");
    expect(initialSynopsisSource("en", false, false)).toBeNull();
  });
});

describe("aniListTarget", () => {
  it("does nothing while the catalog lookup is loading", () => {
    expect(aniListTarget(false, "https://anilist.co/anime/1", "X")).toEqual({ kind: "loading" });
  });
  it("opens the matched AniList entry", () => {
    expect(aniListTarget(true, "https://anilist.co/anime/1", "X")).toEqual({
      kind: "open",
      url: "https://anilist.co/anime/1",
    });
  });
  it("searches AniList by title when unlinked — never the pirate page", () => {
    expect(aniListTarget(true, null, "Bocchi the Rock!")).toEqual({
      kind: "search",
      url: "https://anilist.co/search/anime?search=Bocchi%20the%20Rock!",
    });
    expect(aniListTarget(true, "https://animeyt.cc/tv/bocchi/", "B")).toEqual({
      kind: "search",
      url: aniListSearchUrl("B"),
    });
  });
  it("only accepts real anilist.co https URLs", () => {
    expect(isAniListUrl("https://anilist.co.evil.example/anime/1")).toBe(false);
    expect(isAniListUrl("http://anilist.co/anime/1")).toBe(false);
    expect(isAniListUrl("not a url")).toBe(false);
  });
});

describe("sitePageUrl / seasonOf", () => {
  it("hides the site link for catalog-only rows", () => {
    expect(sitePageUrl("https://anilist.co/anime/5")).toBeNull();
    expect(sitePageUrl("https://animeyt.cc/tv/x/")).toBe("https://animeyt.cc/tv/x/");
    expect(sitePageUrl("javascript:alert(1)")).toBeNull();
  });
  it("maps a premiere date to its AniList season", () => {
    expect(seasonOf(Date.UTC(2024, 0, 6) / 1000)).toEqual({ season: "winter", year: 2024 });
    expect(seasonOf(Date.UTC(2023, 9, 1) / 1000)).toEqual({ season: "fall", year: 2023 });
    expect(seasonOf(null)).toBeNull();
  });
});
