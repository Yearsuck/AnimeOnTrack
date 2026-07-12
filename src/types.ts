export interface Series {
  id: number;
  slug: string;
  title: string;
  url: string;
  cover_url: string | null;
  is_airing: boolean;
  followed: boolean;
  /** Unix timestamp (seconds) of the next episode's release, from the airing
   *  listing's countdown; null when the card has none / never scanned. */
  next_episode_at: number | null;
  /** The site's own episode-count badge; null when non-numeric ("??"). */
  site_episode_count: number | null;
}

export interface Episode {
  id: number;
  series_id: number;
  number: string;
  title: string | null;
  url: string;
  released_at: string | null;
  seen: boolean;
}

export interface PendingItem {
  series: Series;
  episode: Episode;
}

export interface NextEpisode {
  number: string;
  title: string | null;
  url: string;
}

export interface LibraryItem {
  series: Series;
  total_episodes: number;
  seen_episodes: number;
  last_added: string | null;
  /** Lowest-numbered unseen episode, or null when fully seen (or none exist). */
  next_episode: NextEpisode | null;
  /** MAX(episodes.seen_at) — when the user last marked an episode seen. */
  last_watched_at: string | null;
}

export interface GenreStat {
  genre: string;
  count: number;
}

export interface TypeStat {
  kind: string;
  count: number;
}

export interface WatchSummary {
  followed_series: number;
  episodes_watched: number;
  episodes_total: number;
  backlog_want: number;
}

export interface SeriesGraphNode {
  id: number;
  title: string;
  cover_url: string | null;
  genres: string[];
  kind: string | null;
}

export interface SwipeCard {
  title: string;
  url: string;
  poster_url: string | null;
  kind: string;
  matched_genre: string | null;
}

export type SwipeDecision = "Seen" | "Want" | "Discard";

// One entry in the Descubrir swipe-history strip. `decision` is derived live
// from the row's classification flags on the backend ("seen"|"want"|
// "discard"|"none"), so a reclassify in between is reflected on next read.
export interface SwipeHistoryItem {
  series_id: number;
  title: string;
  poster_url: string | null;
  decision: "seen" | "want" | "discard" | "none";
}

// The Descubrir deck's user-configured genre/format bans (global, not
// per-site). Additive to the always-on Hentai/Ecchi baseline.
export interface DeckBans {
  genres: string[];
  formats: string[];
}

export type LinkOutcome =
  | { type: "Linked"; url: string; episodes: number }
  | { type: "NoMatch" }
  | { type: "AlreadyLinked" };

// Mirrors the backend's `Classification` enum (src-tauri/src/commands.rs) —
// the target state for `reclassify_series`, the universal "de-classify /
// move between lists" inverse. Plain Rust variant names on the wire, same
// convention as `SwipeDecision`.
export type Classification = "None" | "Want" | "Discarded" | "WatchedExternally";

export interface GenreAffinity {
  genre: string;
  score: number;
}

export interface SiteSummary {
  id: string;
  name: string;
  default_base_url: string;
}

export interface SiteSwitchResult {
  site: SiteSummary;
  is_first_time: boolean;
}

export interface CatalogAnime {
  id: number;
  title: string;
  cover_url: string | null;
  format: string | null;
  genres: string[];
  episodes: number | null;
  average_score: number | null;
  popularity: number | null;
  url: string;
}

export interface CatalogPage {
  items: CatalogAnime[];
  has_next_page: boolean;
  total_synced: number;
  total_matching: number;
}

export interface CatalogFilter {
  search?: string;
  genres?: string[];
  format?: string;
  min_score?: number;
  episodes?: string;
}

export interface CatalogFacets {
  genres: string[];
  formats: string[];
}

export interface CatalogSyncProgress {
  synced: number;
  total: number;
}
