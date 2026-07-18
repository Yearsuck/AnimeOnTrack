import type { Classification } from "../../types";

// The deck's format whitelist (backend DEFAULT_FORMATS in
// db::random_catalog_anime_in_genre) — the "tipos" the Filtros view can ban.
export const DECK_FORMATS = ["TV", "MOVIE", "OVA", "ONA", "SPECIAL"];

// Map a history-strip re-classify action to its reclassify_series target.
export const RECLASSIFY_TARGET: Record<"discard" | "want" | "seen", Classification> = {
  discard: "Discarded",
  want: "Want",
  seen: "WatchedExternally",
};

// Deck settings panel's collapsed/open state, persisted the same way as
// theme.ts / discoverMode.ts (read once for initial state, write on toggle).
// Local to this file — no other view reads it, so no dedicated module.
export const DECK_PANEL_STORAGE_KEY = "aot.deckPanel";

export const TOP_GENRES_LIMIT = 5;
// Prefetch buffer: keep this many cards ready locally so a decision shows
// the next card instantly instead of waiting on a fresh discover_catalog_card
// round-trip. discover_catalog_card is a local SQLite read (no Cloudflare
// exposure, no network at all), so filling this concurrently is cheap and
// safe — unlike the scraped-site path, there's no rate limit to respect here.
export const PREFETCH_TARGET = 10;
export const REFILL_THRESHOLD = 4;
export const MAX_FILL_ROUNDS = 5;

export const DECISION_BADGE = {
  want: "discover.badgeWant",
  seen: "discover.badgeSeen",
  discard: "discover.badgeDiscard",
  none: "discover.badgeDiscard",
} as const;
