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
