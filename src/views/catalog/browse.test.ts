import { describe, expect, it } from "vitest";
import type { CatalogAnime } from "../../types";
import {
  activeFilterChips,
  countdownParts,
  countdownShape,
  isBrowseFilter,
  isBrowseMode,
  pickThisWeek,
  type CatalogFilterState,
  type ChipLabels,
} from "./browse";

const NOW = 1_000_000;

function anime(id: number, next_airing_at: number | null): CatalogAnime {
  return {
    id,
    title: `A${id}`,
    cover_url: null,
    format: "TV",
    genres: [],
    episodes: null,
    average_score: null,
    popularity: null,
    url: "",
    studio: null,
    next_airing_at,
  };
}

const EMPTY: CatalogFilterState = {
  search: "",
  selectedGenres: [],
  format: "",
  minScore: null,
  maxScore: null,
  minStartDate: null,
  maxStartDate: null,
  status: "",
  sort: "popularity",
  episodes: "",
  studio: "",
  hideWatched: false,
};

const LABELS: ChipLabels = {
  format: (f) => `fmt ${f}`,
  status: (s) => `st ${s}`,
  sort: (s) => `sort ${s}`,
  episodes: (e) => `eps ${e}`,
  score: (min, max) => `score ${min}-${max}`,
  date: (min, max) => `date ${min}-${max}`,
  hideWatched: "hide",
};

describe("pickThisWeek", () => {
  it("keeps only the next 7 days, soonest first, capped", () => {
    const items = [
      anime(1, NOW + 3 * 86400),
      anime(2, NOW - 10), // already aired, stale countdown
      anime(3, NOW + 3600),
      anime(4, NOW + 8 * 86400), // beyond the week
      anime(5, null),
      anime(6, NOW + 2 * 86400),
    ];
    expect(pickThisWeek(items, NOW).map((a) => a.id)).toEqual([3, 6, 1]);
    expect(pickThisWeek(items, NOW, 2).map((a) => a.id)).toEqual([3, 6]);
  });

  it("falls back to the popularity order when nothing has a usable countdown", () => {
    const items = [anime(1, null), anime(2, NOW - 5), anime(3, null)];
    expect(pickThisWeek(items, NOW, 2).map((a) => a.id)).toEqual([1, 2]);
  });
});

describe("countdownParts", () => {
  it("is null once the time has passed", () => {
    expect(countdownParts(NOW, NOW)).toBeNull();
    expect(countdownParts(null, NOW)).toBeNull();
  });

  it("splits into days, hours and minutes, rounding minutes up", () => {
    expect(countdownParts(NOW + 13_151, NOW)).toEqual({ days: 0, hours: 3, minutes: 40 });
    expect(countdownParts(NOW + 99_551, NOW)).toEqual({ days: 1, hours: 3, minutes: 40 });
    expect(countdownParts(NOW + 20, NOW)).toEqual({ days: 0, hours: 0, minutes: 1 });
    expect(countdownParts(NOW + 3600, NOW)).toEqual({ days: 0, hours: 1, minutes: 0 });
  });

  it("picks the largest unit for the label shape", () => {
    expect(countdownShape({ days: 1, hours: 0, minutes: 5 })).toBe("days");
    expect(countdownShape({ days: 0, hours: 2, minutes: 5 })).toBe("hours");
    expect(countdownShape({ days: 0, hours: 0, minutes: 5 })).toBe("minutes");
  });
});

describe("activeFilterChips / isBrowseMode", () => {
  it("an untouched filter is browse mode with no chips", () => {
    expect(activeFilterChips(EMPTY, LABELS)).toEqual([]);
    expect(isBrowseMode(EMPTY)).toBe(true);
  });

  it("a search alone leaves browse mode but is not a chip", () => {
    const s = { ...EMPTY, search: "isekai" };
    expect(activeFilterChips(s, LABELS)).toEqual([]);
    expect(isBrowseMode(s)).toBe(false);
  });

  it("lists every active filter as a removable chip", () => {
    const s: CatalogFilterState = {
      ...EMPTY,
      status: "RELEASING",
      selectedGenres: ["Fantasy", "Drama"],
      minScore: 70,
      sort: "score_desc",
      hideWatched: true,
    };
    const chips = activeFilterChips(s, LABELS);
    expect(chips.map((c) => c.label)).toEqual(["st RELEASING", "Fantasy", "Drama", "score 70-null", "hide", "sort score_desc"]);
    expect(chips[1].key).toEqual({ kind: "genre", genre: "Fantasy" });
    expect(isBrowseMode(s)).toBe(false);
  });

  it("isBrowseFilter matches the request filter Catalog builds", () => {
    expect(isBrowseFilter({ sort: "popularity", search: undefined, genres: undefined })).toBe(true);
    expect(isBrowseFilter({ sort: "score_desc" })).toBe(false);
    expect(isBrowseFilter({ sort: "popularity", status: "RELEASING" })).toBe(false);
    expect(isBrowseFilter({ search: "x" })).toBe(false);
  });

  it("a non-default sort alone leaves browse mode (Ver todo on Mejor valorados)", () => {
    expect(isBrowseMode({ ...EMPTY, sort: "score_desc" })).toBe(false);
    expect(isBrowseMode({ ...EMPTY, sort: "" })).toBe(true);
  });
});
