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

// One airing series plus its parsed first-episode date, for the "Esta
// temporada" filter. Most airing series won't have one — episodes are only
// scraped on-demand (followed/opened), not for the whole catalog — so
// `first_episode_at: null` means "unknown", not "old".
export interface AiringItem {
  series: Series;
  /** Unix timestamp (seconds) of the series' first scraped episode's release
   *  date, or null when unknown (no scraped episodes / unparseable date). */
  first_episode_at: number | null;
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
  /** Mirrors series.watched_externally — a catalog "Ya lo vi" swipe, which
   *  never scrapes episodes (total_episodes stays 0). */
  watched_externally: boolean;
  /** Raw series.kind ("TV"/"MOVIE"/"Pelicula"/"OVA"/"ONA"/"SPECIAL"/site
   *  quality tags/null) — unnormalized, see normalizeKind() in Library.tsx. */
  kind: string | null;
  /** This series' genres from series_genres, sorted. */
  genres: string[];
  /** Only populated for series linked to an AniList catalog row (anilist_id
   *  set) — scraped-only followed series have no native studio data and this
   *  stays null. */
  studio: string | null;
}

export interface GenreStat {
  genre: string;
  count: number;
}

export interface GenreCardSeries {
  title: string;
  cover_url: string | null;
}

export interface GenreCard {
  genre: string;
  count: number;
  top_series: GenreCardSeries[];
}

export interface TypeStat {
  kind: string;
  count: number;
}

export interface WatchSummary {
  followed_series: number;
  distinct_anime: number;
  episodes_watched: number;
  episodes_total: number;
  episodes_watched_external: number;
  airing_followed: number;
  pending_to_watch: number;
  backlog_want: number;
}

export interface SeriesGraphNode {
  id: number;
  title: string;
  cover_url: string | null;
  genres: string[];
  kind: string | null;
}

export interface DayCount {
  day: string;
  count: number;
}

export interface TitleCount {
  title: string;
  count: number;
}

export interface DustyEntry {
  title: string;
  last_seen_at: string;
}

export interface BingeRecord {
  day: string | null;
  count: number;
}

export interface HourCount {
  hour: number;
  count: number;
}

export interface WatchInsights {
  estimated_minutes_tracked: number;
  estimated_minutes_external: number;
  external_titles_estimated: number;
  external_titles_total: number;
  avg_episodes_per_series: number;
  followed_airing: number;
  followed_finished: number;
  discarded: number;
  want: number;
  watched_externally: number;
  top_series: TitleCount[];
  marks_by_day: DayCount[];
  marks_tracked_since: string | null;
}

export interface PopularityBias {
  average_popularity: number | null;
  normalized_score: number | null;
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
  // series.url — lets Descubrir.tsx clear this card from its client-side
  // decided-set (decidedUrlsRef) when it legitimately returns to the deck.
  url: string;
  decision: "seen" | "want" | "discard" | "none";
}

// The Descubrir deck's user-configured genre/format bans (global, not
// per-site). No hardcoded baseline exclusion — purely user-driven.
export interface DeckBans {
  genres: string[];
  formats: string[];
  hide_upcoming: boolean;
  status_data_synced: boolean;
  min_start_date: number | null;
  max_start_date: number | null;
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
  /** The first isMain studio's name, when AniList has one; co-productions
   *  with multiple mains only keep the first — an approximation, not
   *  exhaustive credit data. */
  studio: string | null;
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
  studio?: string;
}

export interface CatalogFacets {
  genres: string[];
  formats: string[];
  studios: string[];
}

export interface CatalogSyncProgress {
  synced: number;
  total: number;
}

export interface BackupStatus {
  configured: boolean;
  connected: boolean;
  last_at: string | null;
  size_bytes: number | null;
}
