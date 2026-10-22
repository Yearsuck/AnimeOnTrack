import type { Series, ListEntryMeta } from "../../../types";
import type { ListTab } from "../types";

export type ViewMode = "grid" | "list";

export type SortKey = "title" | "score_desc" | "newest" | "oldest" | "episodes" | "recent";

export type StatusFilter = "" | "RELEASING" | "FINISHED" | "NOT_YET_RELEASED" | "CANCELLED";

export type QuickFilter = "" | "airing" | "top_rated" | "short";

export interface ListasEntry {
  series: Series;
  meta?: ListEntryMeta;
}

export interface ListasFilterState {
  query: string;
  sortBy: SortKey;
  selectedFormats: string[];
  status: StatusFilter;
  selectedGenres: string[];
  minScore: number | null;
  yearFrom: number | null;
  yearTo: number | null;
  quickFilter: QuickFilter;
}

export const VIEW_MODE_STORAGE_KEY = "aot_listas_view_mode";
export const MIN_SCORE_OPTIONS = [50, 60, 70, 80, 90] as const;

export type { ListTab };
