export interface Series {
  id: number;
  slug: string;
  title: string;
  url: string;
  cover_url: string | null;
  is_airing: boolean;
  followed: boolean;
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
}

export interface CatalogSyncProgress {
  synced: number;
  total: number;
}
