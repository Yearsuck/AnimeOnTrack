import type { CatalogAnime, CatalogFilter } from "../../types";

// Pure helpers behind Explorar's browse mode (hero + shelves) and its
// results-mode filter chips. Kept out of the components so they can be
// tested without rendering.

const WEEK_SECS = 7 * 86400;

/**
 * The hero's "Esta semana" rail: the airing titles whose next episode lands
 * within the coming week, soonest first. `items` arrive by popularity, so the
 * rail stays among titles people actually watch. When nothing in the list has
 * a usable countdown (stale catalog, everything between seasons) it falls back
 * to the most popular airing titles so the hero is never empty.
 */
export function pickThisWeek(items: CatalogAnime[], now: number, limit = 5): CatalogAnime[] {
  const upcoming = items
    .filter((a) => a.next_airing_at != null && a.next_airing_at > now && a.next_airing_at <= now + WEEK_SECS)
    .sort((a, b) => a.next_airing_at! - b.next_airing_at!);
  return (upcoming.length > 0 ? upcoming : items).slice(0, limit);
}

export interface CountdownParts {
  days: number;
  hours: number;
  minutes: number;
}

/** Time left until `airAt` (unix seconds), or null once it has passed. */
export function countdownParts(airAt: number | null | undefined, now: number): CountdownParts | null {
  if (airAt == null || airAt <= now) return null;
  // Whole minutes, rounded up so "0 min" never shows while the episode is ahead.
  const total = Math.ceil((airAt - now) / 60);
  return {
    days: Math.floor(total / 1440),
    hours: Math.floor((total % 1440) / 60),
    minutes: total % 60,
  };
}

/** Which unit pair the hero's big countdown shows: "1 d 3 h", "3 h 39 min" or "12 min". */
export function countdownShape(p: CountdownParts): "days" | "hours" | "minutes" {
  if (p.days > 0) return "days";
  if (p.hours > 0) return "hours";
  return "minutes";
}

/** Every filter field the results mode can show as a removable chip. */
export interface CatalogFilterState {
  search: string;
  selectedGenres: string[];
  format: string;
  minScore: number | null;
  maxScore: number | null;
  minStartDate: number | null;
  maxStartDate: number | null;
  status: string;
  sort: string;
  episodes: string;
  studio: string;
  hideWatched: boolean;
}

export type FilterChipKey =
  | { kind: "genre"; genre: string }
  | { kind: "format" | "score" | "date" | "status" | "sort" | "episodes" | "studio" | "hideWatched" };

export interface FilterChip {
  id: string;
  label: string;
  key: FilterChipKey;
}

export interface ChipLabels {
  format: (f: string) => string;
  status: (s: string) => string;
  sort: (s: string) => string;
  episodes: (e: string) => string;
  score: (min: number | null, max: number | null) => string;
  date: (min: number | null, max: number | null) => string;
  hideWatched: string;
}

/**
 * The active filters as chips, in the order the drawer lists them. The search
 * text is not a chip (it stays visible in its own field), and the default sort
 * is not a filter.
 */
export function activeFilterChips(s: CatalogFilterState, labels: ChipLabels): FilterChip[] {
  const chips: FilterChip[] = [];
  if (s.status) chips.push({ id: "status", label: labels.status(s.status), key: { kind: "status" } });
  for (const g of s.selectedGenres) chips.push({ id: `genre:${g}`, label: g, key: { kind: "genre", genre: g } });
  if (s.format) chips.push({ id: "format", label: labels.format(s.format), key: { kind: "format" } });
  if (s.minScore !== null || s.maxScore !== null)
    chips.push({ id: "score", label: labels.score(s.minScore, s.maxScore), key: { kind: "score" } });
  if (s.minStartDate !== null || s.maxStartDate !== null)
    chips.push({ id: "date", label: labels.date(s.minStartDate, s.maxStartDate), key: { kind: "date" } });
  if (s.episodes) chips.push({ id: "episodes", label: labels.episodes(s.episodes), key: { kind: "episodes" } });
  if (s.studio) chips.push({ id: "studio", label: s.studio, key: { kind: "studio" } });
  if (s.hideWatched) chips.push({ id: "hideWatched", label: labels.hideWatched, key: { kind: "hideWatched" } });
  if (s.sort && s.sort !== "popularity") chips.push({ id: "sort", label: labels.sort(s.sort), key: { kind: "sort" } });
  return chips;
}

/** Browse mode (hero + shelves) shows only while nothing narrows the catalog. */
export function isBrowseMode(s: CatalogFilterState): boolean {
  return s.search === "" && activeFilterChips(s, NO_LABELS).length === 0;
}

/** The same test on the request filter: only the default sort may be set. */
export function isBrowseFilter(f: CatalogFilter): boolean {
  return Object.entries(f).every(([k, v]) => v === undefined || (k === "sort" && v === "popularity"));
}

const NO_LABELS: ChipLabels = {
  format: () => "",
  status: () => "",
  sort: () => "",
  episodes: () => "",
  score: () => "",
  date: () => "",
  hideWatched: "",
};
