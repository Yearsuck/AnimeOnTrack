import { describe, it, expect } from "vitest";
import { NO_GENRE_KEY } from "../../lib/categoryColor";
import type { SeriesGraphNode } from "../../types";
import {
  OTHER_GENRES_KEY,
  assignHubColors,
  buildGenreAtlas,
  declutterLabels,
  escapeHtml,
  fibonacciSphere,
  fitDistance,
  labelStyle,
  hubCounts,
  neighbourhood,
  normalizeKind,
  placeHubs,
  presentKinds,
  scaleSqrt,
  screenSpriteScale,
  searchShows,
  topShowsOf,
  withAlpha,
} from "./graphModel";

const s = (id: number, genres: string[], kind: string | null = "TV", title = `Show ${id}`): SeriesGraphNode => ({
  id,
  title,
  cover_url: null,
  genres,
  kind,
});

const eps = (m: Record<number, number>) => (id: number) => m[id] ?? 0;

describe("buildGenreAtlas", () => {
  it("links each show only to its own genres, with no hub-to-hub or type links", () => {
    const atlas = buildGenreAtlas([s(1, ["Action", "Comedy"], "TV"), s(2, ["Comedy"], "MOVIE")], eps({}));
    expect(atlas.links.map((l) => `${l.hub}>${l.showId}`).sort()).toEqual(["Action>1", "Comedy>1", "Comedy>2"]);
    expect(atlas.hubs.map((h) => h.key).sort()).toEqual(["Action", "Comedy"]);
  });

  it("caps links per show to the first N genres in the given order", () => {
    const atlas = buildGenreAtlas([s(1, ["A", "B", "C", "D", "E"])], eps({}), { maxGenresPerShow: 3 });
    expect(atlas.shows[0].hubs).toEqual(["A", "B", "C"]);
    expect(atlas.links).toHaveLength(3);
  });

  it("folds the long tail into one other-genres hub when there are too many genres", () => {
    const series: SeriesGraphNode[] = [];
    // genre G0 on 20 shows, G1 on 19 ... so the ranking is unambiguous
    let id = 0;
    for (let g = 0; g < 20; g++) {
      for (let k = 0; k < 20 - g; k++) series.push(s(id++, [`G${g}`]));
    }
    const atlas = buildGenreAtlas(series, eps({}), { maxHubs: 5 });
    const keys = atlas.hubs.map((h) => h.key);
    expect(keys).toEqual(["G0", "G1", "G2", "G3", "G4", OTHER_GENRES_KEY]);
    const other = atlas.hubs.find((h) => h.key === OTHER_GENRES_KEY)!;
    expect(other.folded).toHaveLength(15);
    expect(other.count).toBe(series.length - (20 + 19 + 18 + 17 + 16));
  });

  it("does not fold when only one genre would be folded", () => {
    const atlas = buildGenreAtlas([s(1, ["A"]), s(2, ["B"]), s(3, ["C"])], eps({}), { maxHubs: 2 });
    expect(atlas.hubs.map((h) => h.key).sort()).toEqual(["A", "B", "C"]);
  });

  it("deduplicates a show whose several tail genres fold into the same hub", () => {
    const series = [s(1, ["A"]), s(2, ["A"]), s(3, ["B"]), s(4, ["B"]), s(5, ["A", "X", "Y"])];
    const atlas = buildGenreAtlas(series, eps({}), { maxHubs: 1 });
    // ranking: A(3), B(2), X(1), Y(1) -> keep A, fold B/X/Y
    expect(atlas.shows.find((x) => x.id === 5)!.hubs).toEqual(["A", OTHER_GENRES_KEY]);
  });

  it("puts shows without genres on the no-genre hub", () => {
    const atlas = buildGenreAtlas([s(1, [])], eps({}));
    expect(atlas.shows[0].hubs).toEqual([NO_GENRE_KEY]);
    expect(atlas.hubs[0].key).toBe(NO_GENRE_KEY);
  });

  it("colours a show by its primary genre and weights links by episodes within the hub", () => {
    const atlas = buildGenreAtlas([s(1, ["A", "B"]), s(2, ["A"])], eps({ 1: 5, 2: 10 }));
    const a = atlas.hubs.find((h) => h.key === "A")!;
    expect(atlas.shows.find((x) => x.id === 1)!.color).toBe(a.color);
    expect(a.episodes).toBe(15);
    const w = (hub: string, id: number) => atlas.links.find((l) => l.hub === hub && l.showId === id)!.weight;
    expect(w("A", 2)).toBe(1);
    expect(w("A", 1)).toBe(0.5);
    expect(w("B", 1)).toBe(1);
  });

  it("orders hubs by show count with the synthetic buckets last", () => {
    const atlas = buildGenreAtlas([s(1, []), s(2, ["B"]), s(3, ["A", "B"])], eps({}));
    expect(atlas.hubs.map((h) => h.key)).toEqual(["B", "A", NO_GENRE_KEY]);
  });
});

describe("assignHubColors", () => {
  it("gives distinct colours to up to palette-size genres", () => {
    const keys = Array.from({ length: 16 }, (_, i) => `Genre${i}`);
    const colors = assignHubColors(keys);
    expect(new Set(colors.values()).size).toBe(16);
  });
  it("is stable for the same input", () => {
    const keys = ["Action", "Comedy", "Drama"];
    expect([...assignHubColors(keys)]).toEqual([...assignHubColors(keys)]);
  });
});

describe("placement", () => {
  it("spreads points on the sphere at the given radius", () => {
    const pts = fibonacciSphere(10, 100);
    expect(pts).toHaveLength(10);
    for (const p of pts) expect(Math.hypot(p.x, p.y, p.z)).toBeCloseTo(100, 5);
  });

  it("puts genres that share shows closer than genres that share none", () => {
    const series = [
      ...Array.from({ length: 6 }, (_, i) => s(i, ["A", "B"])),
      ...Array.from({ length: 5 }, (_, i) => s(100 + i, ["C"])),
      ...Array.from({ length: 4 }, (_, i) => s(200 + i, ["D"])),
      ...Array.from({ length: 3 }, (_, i) => s(300 + i, ["E"])),
      ...Array.from({ length: 2 }, (_, i) => s(400 + i, ["F"])),
    ];
    const atlas = buildGenreAtlas(series, eps({}));
    const pos = placeHubs(atlas.hubs, atlas.shows, 100);
    const d = (a: string, b: string) => {
      const p = pos.get(a)!;
      const q = pos.get(b)!;
      return Math.hypot(p.x - q.x, p.y - q.y, p.z - q.z);
    };
    for (const other of ["C", "D", "E", "F"]) expect(d("A", "B")).toBeLessThanOrEqual(d("A", other));
    expect(pos.size).toBe(atlas.hubs.length);
  });
});

describe("neighbourhood", () => {
  const atlas = buildGenreAtlas([s(1, ["A", "B"]), s(2, ["B"]), s(3, ["C"])], eps({}));
  it("genre selection lights its hub, its shows and their links to it", () => {
    const n = neighbourhood(atlas, { type: "genre", key: "B" })!;
    expect([...n.nodes].sort()).toEqual(["genre:B", "series:1", "series:2"]);
    expect([...n.links].sort()).toEqual(["B|1", "B|2"]);
  });
  it("show selection lights the show and its genres", () => {
    const n = neighbourhood(atlas, { type: "series", id: 1 })!;
    expect([...n.nodes].sort()).toEqual(["genre:A", "genre:B", "series:1"]);
    expect([...n.links].sort()).toEqual(["A|1", "B|1"]);
  });
  it("nothing selected or unknown show dims nothing", () => {
    expect(neighbourhood(atlas, null)).toBeNull();
    expect(neighbourhood(atlas, { type: "series", id: 99 })).toBeNull();
  });
});

describe("filters and search", () => {
  const atlas = buildGenreAtlas(
    [s(1, ["A"], "TV", "Shingeki no Kyojin"), s(2, ["A"], "Pelicula", "Kimi no Na wa"), s(3, ["B"], null, "Pokémon")],
    eps({ 1: 80, 2: 1 })
  );
  it("normalises scraped kinds", () => {
    expect(normalizeKind("Pelicula")).toBe("MOVIE");
    expect(normalizeKind("tv")).toBe("TV");
    expect(normalizeKind("4K")).toBe("OTHER");
    expect(normalizeKind(null)).toBe("OTHER");
    expect(presentKinds(atlas)).toEqual(["TV", "MOVIE", "OTHER"]);
  });
  it("counts shows per hub under a type filter", () => {
    expect(hubCounts(atlas, "all").get("A")).toBe(2);
    expect(hubCounts(atlas, "MOVIE").get("A")).toBe(1);
    expect(hubCounts(atlas, "MOVIE").get("B")).toBe(0);
  });
  it("searches titles ignoring case and accents, prefix first", () => {
    expect(searchShows(atlas.shows, "pokemon").map((x) => x.id)).toEqual([3]);
    expect(searchShows(atlas.shows, "NO").map((x) => x.id)).toEqual([1, 2]);
    expect(searchShows(atlas.shows, "kimi").map((x) => x.id)).toEqual([2]);
    expect(searchShows(atlas.shows, "  ")).toEqual([]);
  });
  it("lists a genre's most watched shows", () => {
    expect(topShowsOf(atlas, "A", "all").map((x) => x.id)).toEqual([1, 2]);
    expect(topShowsOf(atlas, "A", "MOVIE").map((x) => x.id)).toEqual([2]);
  });
});

describe("helpers", () => {
  it("escapes HTML in tooltip text", () => {
    expect(escapeHtml(`<img src=x onerror="a">&'`)).toBe("&lt;img src=x onerror=&quot;a&quot;&gt;&amp;&#39;");
  });
  it("scales with a square-root curve inside the bounds", () => {
    expect(scaleSqrt(0, 100, 10, 30)).toBe(10);
    expect(scaleSqrt(100, 100, 10, 30)).toBe(30);
    expect(scaleSqrt(25, 100, 10, 30)).toBe(20);
    expect(scaleSqrt(5, 0, 10, 30)).toBe(10);
  });
  it("fits a sphere to the narrower side of the free area", () => {
    // 90deg vertical fov, square free area: half-angle 45deg -> r / sin(45deg)
    expect(fitDistance(100, 90, 500, 500, 1)).toBeCloseTo(100 / Math.SQRT1_2, 6);
    // a wide area is limited by its height, a narrow one by its width
    expect(fitDistance(100, 90, 2000, 500, 1)).toBeCloseTo(100 / Math.SQRT1_2, 6);
    expect(fitDistance(100, 90, 250, 500, 1)).toBeGreaterThan(fitDistance(100, 90, 500, 500, 1));
  });
  it("shrinks labels of small genres and drops tiny counts", () => {
    expect(labelStyle(100, 100)).toEqual({ px: 26, fontPx: 13, showCount: true });
    expect(labelStyle(10, 100).px).toBeLessThan(26);
    expect(labelStyle(10, 100).showCount).toBe(true);
    expect(labelStyle(2, 100).showCount).toBe(false);
  });
  it("lifts overlapping labels above the bigger genre's label, which never moves", () => {
    const lift = declutterLabels([
      { key: "Adventure", x: 110, y: 200, w: 100, h: 26, priority: 105 },
      { key: "Action", x: 100, y: 205, w: 100, h: 26, priority: 127 },
      { key: "Far", x: 600, y: 200, w: 100, h: 26, priority: 1 },
    ]);
    expect(lift.get("Action")).toBe(0);
    expect(lift.get("Far")).toBe(0);
    // Adventure's bottom must end above Action's top (205 - 26 = 179) with the gap.
    expect(200 - lift.get("Adventure")!).toBeLessThanOrEqual(179 - 2);
  });
  it("stacks a third overlapping label above both", () => {
    const lift = declutterLabels([
      { key: "A", x: 100, y: 200, w: 80, h: 20, priority: 3 },
      { key: "B", x: 100, y: 200, w: 80, h: 20, priority: 2 },
      { key: "C", x: 100, y: 200, w: 80, h: 20, priority: 1 },
    ]);
    expect(lift.get("A")).toBe(0);
    expect(lift.get("B")).toBe(22);
    expect(lift.get("C")).toBe(44);
  });
  it("adds alpha to hex colours", () => {
    expect(withAlpha("#ff0000", 0.5)).toBe("rgba(255, 0, 0, 0.5)");
    expect(withAlpha("#0f0", 1)).toBe("rgba(0, 255, 0, 1)");
    expect(withAlpha("rgb(1, 2, 3)", 0.2)).toBe("rgb(1, 2, 3)");
  });
  it("keeps a sprite a fixed pixel height", () => {
    // fov 90deg -> view height at distance 1 is 2 units; 100px of a 1000px viewport = 0.2
    expect(screenSpriteScale(100, 90, 1000)).toBeCloseTo(0.2, 6);
    expect(screenSpriteScale(10, 50, 0)).toBe(0);
  });
});
