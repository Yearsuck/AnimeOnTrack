import type { LibraryItem } from "../types";

/**
 * Picks candidates for the "Continuar viendo" strip at the top of the Pendientes tab.
 *
 * Keeps items that:
 * - Have a next episode to watch (`next_episode !== null`)
 * - Have been watched before within the app (`last_watched_at !== null` and `seen_episodes > 0`)
 * - Were not marked externally via "Ya lo vi" (`!watched_externally`)
 *
 * Sorted by `last_watched_at` descending (most recently watched first).
 * Ties are broken by series title ascending (case-insensitive).
 */
export function pickContinueWatching(items: LibraryItem[], limit = 3): LibraryItem[] {
  if (!items || limit <= 0) return [];

  const candidates = items.filter(
    (item) =>
      item.next_episode !== null &&
      item.last_watched_at !== null &&
      !item.watched_externally &&
      item.seen_episodes > 0
  );

  candidates.sort((a, b) => {
    // Both last_watched_at are non-null here due to the filter above.
    const diff = (b.last_watched_at ?? "").localeCompare(a.last_watched_at ?? "");
    if (diff !== 0) return diff;
    return (
      a.series.title.toLowerCase().localeCompare(b.series.title.toLowerCase()) ||
      a.series.title.localeCompare(b.series.title)
    );
  });

  return candidates.slice(0, limit);
}
