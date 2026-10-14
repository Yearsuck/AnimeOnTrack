/**
 * Default values for the Descubrir swipe deck filters and niche slider.
 * Used when resetting the deck panel to its factory defaults.
 */
export const DECK_DEFAULTS = {
  genres: [] as string[],
  formats: [] as string[],
  hideUpcoming: false,
  minStartDate: null as number | null,
  maxStartDate: null as number | null,
  minScore: null as number | null,
  nicheBias: 0,
};

/**
 * Counts the active bans/filters configured in the deck options panel.
 *
 * Each banned genre, banned format, score floor, date constraint (either or
 * both bounds count as one filter), and hide-upcoming flag adds 1.
 * Accepts any Iterable<string> (Array, Set, etc.) for genres and formats.
 */
export function countActiveFilters(s: {
  genres: Iterable<string>;
  formats: Iterable<string>;
  hideUpcoming: boolean;
  minStartDate: number | null;
  maxStartDate: number | null;
  minScore: number | null;
}): number {
  const countIterable = (items: Iterable<string>): number => {
    if (Array.isArray(items)) return items.length;
    if (items instanceof Set) return items.size;
    let count = 0;
    for (const _ of items) count++;
    return count;
  };

  const genresCount = countIterable(s.genres);
  const formatsCount = countIterable(s.formats);
  const scoreCount = s.minScore !== null ? 1 : 0;
  const dateCount = s.minStartDate !== null || s.maxStartDate !== null ? 1 : 0;
  const hideUpcomingCount = s.hideUpcoming ? 1 : 0;

  return genresCount + formatsCount + scoreCount + dateCount + hideUpcomingCount;
}
