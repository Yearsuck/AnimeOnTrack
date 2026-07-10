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

export interface LibraryItem {
  series: Series;
  total_episodes: number;
  seen_episodes: number;
  last_added: string | null;
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

export type LinkOutcome =
  | { type: "Linked"; url: string; episodes: number }
  | { type: "NoMatch" }
  | { type: "AlreadyLinked" };

export interface GenreAffinity {
  genre: string;
  score: number;
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
