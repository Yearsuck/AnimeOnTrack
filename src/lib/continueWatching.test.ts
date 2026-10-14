import { describe, expect, it } from "vitest";
import { pickContinueWatching } from "./continueWatching";
import type { LibraryItem, Series } from "../types";

function makeLibraryItem(overrides: Partial<LibraryItem> = {}): LibraryItem {
  const { series: seriesOverrides, ...restOverrides } = overrides;
  const series: Series = {
    id: 1,
    slug: "series-slug",
    title: "Series Title",
    url: "https://example.com/series/slug",
    cover_url: null,
    is_airing: true,
    followed: true,
    next_episode_at: null,
    site_episode_count: 12,
    ...seriesOverrides,
  };

  return {
    series,
    total_episodes: 12,
    seen_episodes: 1,
    last_added: "2026-10-01 12:00:00",
    next_episode: {
      number: "2",
      title: "Episode 2",
      url: "https://example.com/watch/series-2",
    },
    last_watched_at: "2026-10-10 12:00:00",
    watched_externally: false,
    kind: "TV",
    genres: ["Action"],
    studio: "Mappa",
    ...restOverrides,
  };
}

describe("pickContinueWatching", () => {
  it("returns empty array for empty input", () => {
    expect(pickContinueWatching([])).toEqual([]);
  });

  it("orders by most recent first", () => {
    const older = makeLibraryItem({
      series: {
        id: 1,
        slug: "older",
        title: "Older Series",
        url: "",
        cover_url: null,
        is_airing: true,
        followed: true,
        next_episode_at: null,
        site_episode_count: 12,
      },
      last_watched_at: "2026-10-05 10:00:00",
    });
    const newer = makeLibraryItem({
      series: {
        id: 2,
        slug: "newer",
        title: "Newer Series",
        url: "",
        cover_url: null,
        is_airing: true,
        followed: true,
        next_episode_at: null,
        site_episode_count: 12,
      },
      last_watched_at: "2026-10-12 18:30:00",
    });

    const result = pickContinueWatching([older, newer]);
    expect(result.map((it) => it.series.id)).toEqual([2, 1]);
  });

  it("drops items without a next episode", () => {
    const item = makeLibraryItem({ next_episode: null });
    expect(pickContinueWatching([item])).toEqual([]);
  });

  it("drops items without last_watched_at", () => {
    const item = makeLibraryItem({ last_watched_at: null });
    expect(pickContinueWatching([item])).toEqual([]);
  });

  it("drops items watched_externally", () => {
    const item = makeLibraryItem({ watched_externally: true });
    expect(pickContinueWatching([item])).toEqual([]);
  });

  it("drops items with seen_episodes === 0", () => {
    const item = makeLibraryItem({ seen_episodes: 0 });
    expect(pickContinueWatching([item])).toEqual([]);
  });

  it("respects default limit of 2", () => {
    const items = [1, 2, 3, 4, 5].map((i) =>
      makeLibraryItem({
        series: {
          id: i,
          slug: `series-${i}`,
          title: `Series ${i}`,
          url: "",
          cover_url: null,
          is_airing: true,
          followed: true,
          next_episode_at: null,
          site_episode_count: 12,
        },
        last_watched_at: `2026-10-0${i} 12:00:00`,
      })
    );
    const result = pickContinueWatching(items);
    expect(result).toHaveLength(2);
    expect(result.map((it) => it.series.id)).toEqual([5, 4]);
  });

  it("returns exactly 2 newest items with 4 eligible items and no explicit limit", () => {
    const items = [1, 2, 3, 4].map((i) =>
      makeLibraryItem({
        series: {
          id: i,
          slug: `series-${i}`,
          title: `Series ${i}`,
          url: "",
          cover_url: null,
          is_airing: true,
          followed: true,
          next_episode_at: null,
          site_episode_count: 12,
        },
        last_watched_at: `2026-10-0${i} 12:00:00`,
      })
    );
    const result = pickContinueWatching(items);
    expect(result).toHaveLength(2);
    expect(result.map((it) => it.series.id)).toEqual([4, 3]);
  });

  it("respects explicit limit of 1", () => {
    const items = [1, 2, 3].map((i) =>
      makeLibraryItem({
        series: {
          id: i,
          slug: `series-${i}`,
          title: `Series ${i}`,
          url: "",
          cover_url: null,
          is_airing: true,
          followed: true,
          next_episode_at: null,
          site_episode_count: 12,
        },
        last_watched_at: `2026-10-0${i} 12:00:00`,
      })
    );
    const result = pickContinueWatching(items, 1);
    expect(result).toHaveLength(1);
    expect(result[0].series.id).toBe(3);
  });

  it("breaks ties by title ascending case-insensitively", () => {
    const itemB = makeLibraryItem({
      series: {
        id: 1,
        slug: "b",
        title: "bleach",
        url: "",
        cover_url: null,
        is_airing: true,
        followed: true,
        next_episode_at: null,
        site_episode_count: 12,
      },
      last_watched_at: "2026-10-10 12:00:00",
    });
    const itemA = makeLibraryItem({
      series: {
        id: 2,
        slug: "a",
        title: "Attack on Titan",
        url: "",
        cover_url: null,
        is_airing: true,
        followed: true,
        next_episode_at: null,
        site_episode_count: 12,
      },
      last_watched_at: "2026-10-10 12:00:00",
    });

    const result = pickContinueWatching([itemB, itemA]);
    expect(result.map((it) => it.series.id)).toEqual([2, 1]);
  });
});
