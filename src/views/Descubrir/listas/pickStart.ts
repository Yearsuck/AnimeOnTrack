import type { ListasEntry } from "./types";

export type StartReason = "airing" | "short" | "top";

export interface StartPick {
  entry: ListasEntry;
  reason: StartReason;
}

/**
 * "Para empezar hoy": a few titles from Quiero ver worth starting now. Airing
 * shows first (they are current and keep giving new episodes), then short
 * ones (≤ 13 episodes, an easy commitment), then simply the best rated; each
 * group by score. Titles not out yet are left out: there is nothing to start.
 */
export function pickStartToday(entries: ListasEntry[], count = 3): StartPick[] {
  const score = (e: ListasEntry) => e.meta?.score ?? 0;
  const byScore = (a: ListasEntry, b: ListasEntry) => score(b) - score(a);
  const startable = entries.filter((e) => e.meta?.status !== "NOT_YET_RELEASED");
  const isAiring = (e: ListasEntry) => e.meta?.status === "RELEASING" || e.series.is_airing;
  // An airing show's site count is only what has aired so far, not its length.
  const episodes = (e: ListasEntry) => e.meta?.episodes ?? (isAiring(e) ? null : e.series.site_episode_count ?? null);

  const picks: StartPick[] = [];
  const taken = new Set<number>();
  // Each group adds up to `max` titles not already picked.
  const take = (list: ListasEntry[], reason: StartReason, max = count) => {
    let added = 0;
    for (const entry of [...list].sort(byScore)) {
      if (picks.length >= count || added >= max) return;
      if (taken.has(entry.series.id)) continue;
      taken.add(entry.series.id);
      picks.push({ entry, reason });
      added++;
    }
  };
  // One airing slot only, so a list full of airing shows still surfaces a
  // short or top-rated one.
  take(startable.filter(isAiring), "airing", 1);
  take(
    startable.filter((e) => {
      const n = episodes(e);
      return n != null && n > 0 && n <= 13;
    }),
    "short"
  );
  take(startable, "top");
  return picks;
}
