import { describe, expect, it } from "vitest";
import type { Series } from "../../../types";
import { pickStartToday } from "./pickStart";
import type { ListasEntry } from "./types";

function entry(
  id: number,
  meta: { score?: number | null; status?: string | null; episodes?: number | null },
  airing = false
): ListasEntry {
  return {
    series: { id, title: `S${id}`, is_airing: airing, site_episode_count: null } as unknown as Series,
    meta: {
      series_id: id,
      anilist_id: id,
      score: meta.score ?? null,
      format: "TV",
      status: meta.status ?? "FINISHED",
      episodes: meta.episodes ?? null,
      start_date: null,
      studio: null,
      popularity: null,
      genres: [],
      cover_url: null,
    },
  };
}

describe("pickStartToday", () => {
  it("leads with the best airing title, then short ones, then the best rated", () => {
    const picks = pickStartToday([
      entry(1, { score: 70, status: "RELEASING" }),
      entry(2, { score: 85, status: "RELEASING" }),
      entry(3, { score: 75, episodes: 12 }),
      entry(4, { score: 90, episodes: 64 }),
      entry(5, { score: 60, episodes: 10 }),
    ]);
    expect(picks.map((p) => [p.entry.series.id, p.reason])).toEqual([
      [2, "airing"],
      [3, "short"],
      [5, "short"],
    ]);
  });

  it("fills with the best rated when there is nothing airing or short", () => {
    const picks = pickStartToday([entry(1, { score: 70, episodes: 24 }), entry(2, { score: 88, episodes: 50 })]);
    expect(picks.map((p) => [p.entry.series.id, p.reason])).toEqual([
      [2, "top"],
      [1, "top"],
    ]);
  });

  it("does not call an airing show short from the episodes aired so far", () => {
    const airing = entry(1, { score: 80, status: "RELEASING" });
    const airing2 = entry(2, { score: 70, status: "RELEASING" });
    (airing2.series as { site_episode_count: number | null }).site_episode_count = 5;
    const picks = pickStartToday([airing, airing2], 3);
    expect(picks.map((p) => [p.entry.series.id, p.reason])).toEqual([
      [1, "airing"],
      [2, "top"],
    ]);
  });

  it("leaves out titles not released yet and never repeats one", () => {
    const picks = pickStartToday(
      [entry(1, { score: 99, status: "NOT_YET_RELEASED", episodes: 12 }), entry(2, { score: 80, episodes: 12 }, true)],
      3
    );
    expect(picks.map((p) => p.entry.series.id)).toEqual([2]);
  });
});
