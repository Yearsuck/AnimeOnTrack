import { useCallback, useMemo, useState } from "react";
import { norm } from "../helpers";
import { sortFormats } from "../../../lib/formatLabel";
import type { ListasEntry, ListasFilterState, QuickFilter, SortKey, StatusFilter } from "./types";

const INITIAL_FILTER_STATE: ListasFilterState = {
  query: "",
  sortBy: "title",
  selectedFormats: [],
  status: "",
  selectedGenres: [],
  minScore: null,
  yearFrom: null,
  yearTo: null,
  quickFilter: "",
};

export function useListasFilter(entries: ListasEntry[]) {
  const [filterState, setFilterState] = useState<ListasFilterState>(INITIAL_FILTER_STATE);

  const setQuery = useCallback((query: string) => {
    setFilterState((prev) => ({ ...prev, query }));
  }, []);

  const setSortBy = useCallback((sortBy: SortKey) => {
    setFilterState((prev) => ({ ...prev, sortBy }));
  }, []);

  // One format at a time, from a select — the same control Explorar uses.
  const setFormat = useCallback((format: string) => {
    setFilterState((prev) => ({ ...prev, selectedFormats: format ? [format] : [] }));
  }, []);

  const setStatus = useCallback((status: StatusFilter) => {
    setFilterState((prev) => ({ ...prev, status }));
  }, []);

  const toggleGenre = useCallback((genre: string) => {
    setFilterState((prev) => {
      const exists = prev.selectedGenres.includes(genre);
      return {
        ...prev,
        selectedGenres: exists
          ? prev.selectedGenres.filter((g) => g !== genre)
          : [...prev.selectedGenres, genre],
      };
    });
  }, []);

  const setMinScore = useCallback((minScore: number | null) => {
    setFilterState((prev) => ({ ...prev, minScore }));
  }, []);

  const setYearFrom = useCallback((yearFrom: number | null) => {
    setFilterState((prev) => ({ ...prev, yearFrom }));
  }, []);

  const setYearTo = useCallback((yearTo: number | null) => {
    setFilterState((prev) => ({ ...prev, yearTo }));
  }, []);

  const toggleQuickFilter = useCallback((qf: QuickFilter) => {
    setFilterState((prev) => ({
      ...prev,
      quickFilter: prev.quickFilter === qf ? "" : qf,
    }));
  }, []);

  const resetFilters = useCallback(() => {
    setFilterState(INITIAL_FILTER_STATE);
  }, []);

  // Compute available genres and their frequencies
  const { availableGenres, topGenres } = useMemo(() => {
    const counts: Record<string, number> = {};
    for (const entry of entries) {
      const list = entry.meta?.genres ?? [];
      for (const g of list) {
        counts[g] = (counts[g] || 0) + 1;
      }
    }
    const all = Object.keys(counts).sort((a, b) => a.localeCompare(b));
    const top = Object.entries(counts)
      .sort((a, b) => b[1] - a[1])
      .slice(0, 10)
      .map(([g]) => g);
    return { availableGenres: all, topGenres: top };
  }, [entries]);

  // Formats actually present in this list (AniList codes, incl. TV_SHORT/MUSIC).
  const availableFormats = useMemo(
    () => sortFormats(entries.flatMap((e) => (e.meta?.format ? [e.meta.format] : []))),
    [entries]
  );

  // Compute smart group counts
  const smartCounts = useMemo(() => {
    let airing = 0;
    let topRated = 0;
    let short = 0;

    for (const { series, meta } of entries) {
      if (meta?.status === "RELEASING" || series.is_airing) {
        airing++;
      }
      if (meta?.score != null && meta.score >= 80) {
        topRated++;
      }
      const eps = meta?.episodes ?? series.site_episode_count;
      if (eps != null && eps > 0 && eps <= 13) {
        short++;
      }
    }

    return { airing, topRated, short };
  }, [entries]);

  // Active filter count
  const activeFilterCount = useMemo(() => {
    let count = 0;
    if (filterState.query.trim()) count++;
    if (filterState.selectedFormats.length > 0) count += filterState.selectedFormats.length;
    if (filterState.status) count++;
    if (filterState.selectedGenres.length > 0) count += filterState.selectedGenres.length;
    if (filterState.minScore != null) count++;
    if (filterState.yearFrom != null || filterState.yearTo != null) count++;
    if (filterState.quickFilter) count++;
    if (filterState.sortBy !== "title") count++;
    return count;
  }, [filterState]);

  // Filtered and sorted entries
  const filteredEntries = useMemo(() => {
    const q = norm(filterState.query.trim());

    const result = entries.filter(({ series, meta }) => {
      // 1. Text Search
      if (q) {
        const titleMatch = norm(series.title).includes(q);
        const genreMatch = meta?.genres?.some((g) => norm(g).includes(q));
        const studioMatch = meta?.studio ? norm(meta.studio).includes(q) : false;
        if (!titleMatch && !genreMatch && !studioMatch) {
          return false;
        }
      }

      // 2. Format filter
      if (filterState.selectedFormats.length > 0) {
        if (!meta?.format || !filterState.selectedFormats.includes(meta.format)) {
          return false;
        }
      }

      // 3. Status filter
      if (filterState.status) {
        if (filterState.status === "RELEASING") {
          const isAiring = meta?.status === "RELEASING" || series.is_airing;
          if (!isAiring) return false;
        } else if (filterState.status === "FINISHED") {
          const isFinished = meta?.status === "FINISHED" || (!series.is_airing && meta?.status == null);
          if (!isFinished) return false;
        } else {
          if (meta?.status !== filterState.status) {
            return false;
          }
        }
      }

      // 4. Genre multi-select filter (AND logic)
      if (filterState.selectedGenres.length > 0) {
        const entryGenres = meta?.genres ?? [];
        const hasAllGenres = filterState.selectedGenres.every((g) => entryGenres.includes(g));
        if (!hasAllGenres) {
          return false;
        }
      }

      // 5. Min score filter
      if (filterState.minScore != null) {
        if (meta?.score == null || meta.score < filterState.minScore) {
          return false;
        }
      }

      // 6. Year range filter
      if (filterState.yearFrom != null || filterState.yearTo != null) {
        if (meta?.start_date == null) {
          return false;
        }
        const year = new Date(meta.start_date * 1000).getUTCFullYear();
        if (filterState.yearFrom != null && year < filterState.yearFrom) {
          return false;
        }
        if (filterState.yearTo != null && year > filterState.yearTo) {
          return false;
        }
      }

      // 7. Quick filters
      if (filterState.quickFilter === "airing") {
        const isAiring = meta?.status === "RELEASING" || series.is_airing;
        if (!isAiring) return false;
      } else if (filterState.quickFilter === "top_rated") {
        if (meta?.score == null || meta.score < 80) return false;
      } else if (filterState.quickFilter === "short") {
        const eps = meta?.episodes ?? series.site_episode_count;
        if (eps == null || eps <= 0 || eps > 13) return false;
      }

      return true;
    });

    // Sort
    result.sort((a, b) => {
      switch (filterState.sortBy) {
        case "title":
          return a.series.title.localeCompare(b.series.title);
        case "score_desc": {
          const scoreA = a.meta?.score ?? -1;
          const scoreB = b.meta?.score ?? -1;
          return scoreB - scoreA || a.series.title.localeCompare(b.series.title);
        }
        case "newest": {
          const dateA = a.meta?.start_date ?? -1;
          const dateB = b.meta?.start_date ?? -1;
          return dateB - dateA || a.series.title.localeCompare(b.series.title);
        }
        case "oldest": {
          const dateA = a.meta?.start_date ?? 99999999999;
          const dateB = b.meta?.start_date ?? 99999999999;
          return dateA - dateB || a.series.title.localeCompare(b.series.title);
        }
        case "episodes": {
          const epsA = a.meta?.episodes ?? a.series.site_episode_count ?? 0;
          const epsB = b.meta?.episodes ?? b.series.site_episode_count ?? 0;
          return epsB - epsA || a.series.title.localeCompare(b.series.title);
        }
        case "recent":
          return b.series.id - a.series.id;
        default:
          return 0;
      }
    });

    return result;
  }, [entries, filterState]);

  return {
    filterState,
    setQuery,
    setSortBy,
    setFormat,
    setStatus,
    toggleGenre,
    setMinScore,
    setYearFrom,
    setYearTo,
    toggleQuickFilter,
    resetFilters,
    availableGenres,
    availableFormats,
    topGenres,
    smartCounts,
    activeFilterCount,
    filteredEntries,
  };
}
