import { useCallback, useEffect, useMemo, useState } from "react";
import { getSeriesGenresBatch, listBacklog, listWatchedExternally } from "../../api";
import { useT } from "../../i18n";
import type { Series } from "../../types";
import type { ListTab } from "./types";
import { norm } from "./helpers";
import { DiscardedRow, WantRow, WatchedRow } from "./rows";

export function ListasView({ onOpenSeries }: { onOpenSeries: (s: Series) => void }) {
  const t = useT();
  const [want, setWant] = useState<Series[]>([]);
  const [discarded, setDiscarded] = useState<Series[]>([]);
  const [watched, setWatched] = useState<Series[]>([]);
  const [genresMap, setGenresMap] = useState<Record<number, string[]>>({});
  const [tab, setTab] = useState<ListTab>("want");
  const [query, setQuery] = useState("");
  const [selectedGenre, setSelectedGenre] = useState<string>("");
  const [airingFilter, setAiringFilter] = useState<"all" | "airing" | "finished">("all");
  const [sortBy, setSortBy] = useState<"title" | "eps_desc" | "eps_asc">("title");

  const load = useCallback(async () => {
    const [w, d, wa] = await Promise.all([
      listBacklog("want"),
      listBacklog("discarded"),
      listWatchedExternally(),
    ]);
    setWant(w);
    setDiscarded(d);
    setWatched(wa);

    const allIds = Array.from(new Set([...w, ...d, ...wa].map((s) => s.id)));
    if (allIds.length > 0) {
      try {
        const gMap = await getSeriesGenresBatch(allIds);
        setGenresMap(gMap);
      } catch (err) {
        console.error("Failed to load series genres batch", err);
      }
    } else {
      setGenresMap({});
    }
  }, []);

  useEffect(() => {
    load();
  }, [load]);

  const currentRawList = useMemo(() => {
    if (tab === "want") return want;
    if (tab === "discarded") return discarded;
    return watched;
  }, [tab, want, discarded, watched]);

  const availableGenres = useMemo(() => {
    const counts: Record<string, number> = {};
    for (const s of currentRawList) {
      const list = genresMap[s.id] || [];
      for (const g of list) {
        counts[g] = (counts[g] || 0) + 1;
      }
    }
    return Object.keys(counts).sort((a, b) => a.localeCompare(b));
  }, [currentRawList, genresMap]);

  const topGenres = useMemo(() => {
    const counts: Record<string, number> = {};
    for (const s of currentRawList) {
      const list = genresMap[s.id] || [];
      for (const g of list) {
        counts[g] = (counts[g] || 0) + 1;
      }
    }
    return Object.entries(counts)
      .sort((a, b) => b[1] - a[1])
      .slice(0, 8)
      .map(([g]) => g);
  }, [currentRawList, genresMap]);

  const filterAndSort = useCallback(
    (list: Series[]) => {
      const q = norm(query.trim());
      let filtered = list;

      if (q) {
        filtered = filtered.filter((s) => {
          if (norm(s.title).includes(q)) return true;
          const gList = genresMap[s.id];
          if (gList && gList.some((g) => norm(g).includes(q))) return true;
          return false;
        });
      }

      if (selectedGenre) {
        filtered = filtered.filter((s) => {
          const gList = genresMap[s.id];
          return gList != null && gList.includes(selectedGenre);
        });
      }

      if (airingFilter === "airing") {
        filtered = filtered.filter((s) => Boolean(s.is_airing));
      } else if (airingFilter === "finished") {
        filtered = filtered.filter((s) => !s.is_airing);
      }

      const sorted = [...filtered];
      if (sortBy === "title") {
        sorted.sort((a, b) => a.title.localeCompare(b.title));
      } else if (sortBy === "eps_desc") {
        sorted.sort((a, b) => (b.site_episode_count ?? 0) - (a.site_episode_count ?? 0));
      } else if (sortBy === "eps_asc") {
        sorted.sort((a, b) => (a.site_episode_count ?? 0) - (b.site_episode_count ?? 0));
      }

      return sorted;
    },
    [query, selectedGenre, airingFilter, sortBy, genresMap]
  );

  const fWant = useMemo(() => filterAndSort(want), [filterAndSort, want]);
  const fDiscarded = useMemo(() => filterAndSort(discarded), [filterAndSort, discarded]);
  const fWatched = useMemo(() => filterAndSort(watched), [filterAndSort, watched]);

  const hasActiveFilters = Boolean(
    query.trim() || selectedGenre || airingFilter !== "all" || sortBy !== "title"
  );

  const handleResetFilters = () => {
    setQuery("");
    setSelectedGenre("");
    setAiringFilter("all");
    setSortBy("title");
  };

  const tabs: { key: ListTab; label: string; count: number }[] = [
    { key: "want", label: t("discover.wantHeading"), count: fWant.length },
    { key: "discarded", label: t("discover.discardedHeading"), count: fDiscarded.length },
    { key: "watched", label: t("discover.watchedHeading"), count: fWatched.length },
  ];

  return (
    <>
      <div className="listas-toolbar">
        <div className="search" style={{ flex: "1 1 220px" }}>
          <span className="icon" aria-hidden="true">
            ⌕
          </span>
          <label htmlFor="listas-search" className="sr-only">
            {t("discover.searchAriaLabel")}
          </label>
          <input
            id="listas-search"
            className="input"
            placeholder={t("discover.searchPlaceholderListas")}
            value={query}
            onChange={(e) => setQuery(e.target.value)}
          />
        </div>

        <select
          className="select filter-select"
          value={selectedGenre}
          onChange={(e) => setSelectedGenre(e.target.value)}
          aria-label={t("discover.allThemes")}
        >
          <option value="">{t("discover.allThemes")}</option>
          {availableGenres.map((g) => (
            <option key={g} value={g}>
              {g}
            </option>
          ))}
        </select>

        <select
          className="select filter-select"
          value={airingFilter}
          onChange={(e) => setAiringFilter(e.target.value as "all" | "airing" | "finished")}
          aria-label={t("discover.filterAllAiring")}
        >
          <option value="all">{t("discover.filterAllAiring")}</option>
          <option value="airing">{t("discover.filterAiring")}</option>
          <option value="finished">{t("discover.filterFinished")}</option>
        </select>

        <select
          className="select filter-select"
          value={sortBy}
          onChange={(e) => setSortBy(e.target.value as "title" | "eps_desc" | "eps_asc")}
          aria-label={t("discover.sortTitle")}
        >
          <option value="title">{t("discover.sortTitle")}</option>
          <option value="eps_desc">{t("discover.sortEpsDesc")}</option>
          <option value="eps_asc">{t("discover.sortEpsAsc")}</option>
        </select>

        {hasActiveFilters && (
          <button
            type="button"
            className="btn btn-sm"
            onClick={handleResetFilters}
          >
            {t("discover.clearFilters")}
          </button>
        )}
      </div>

      {topGenres.length > 0 && (
        <div className="chip-row" style={{ marginBottom: "var(--sp-3)" }}>
          {topGenres.map((g) => {
            const active = selectedGenre === g;
            return (
              <button
                key={g}
                type="button"
                className={`chip-toggle${active ? " active" : ""}`}
                onClick={() => setSelectedGenre(active ? "" : g)}
              >
                {g}
              </button>
            );
          })}
        </div>
      )}

      <div className="seg listas-seg" role="tablist">
        {tabs.map((tb) => (
          <button
            key={tb.key}
            type="button"
            role="tab"
            aria-selected={tab === tb.key}
            className={`seg-btn${tab === tb.key ? " active" : ""}`}
            onClick={() => setTab(tb.key)}
          >
            {tb.label}
            <span className="listas-seg-count">{tb.count}</span>
          </button>
        ))}
      </div>

      {tab === "want" &&
        (want.length === 0 ? (
          <div className="empty">{t("discover.wantEmpty")}</div>
        ) : fWant.length === 0 ? (
          <div className="empty">{t("discover.searchNoResults")}</div>
        ) : (
          <div className="listas-grid">
            {fWant.map((s) => (
              <WantRow
                key={s.id}
                series={s}
                genres={genresMap[s.id]}
                onChanged={load}
                onOpenSeries={onOpenSeries}
              />
            ))}
          </div>
        ))}

      {tab === "discarded" &&
        (discarded.length === 0 ? (
          <div className="empty">{t("discover.discardedEmpty")}</div>
        ) : fDiscarded.length === 0 ? (
          <div className="empty">{t("discover.searchNoResults")}</div>
        ) : (
          <div className="listas-grid">
            {fDiscarded.map((s) => (
              <DiscardedRow
                key={s.id}
                series={s}
                genres={genresMap[s.id]}
                onChanged={load}
                onOpenSeries={onOpenSeries}
              />
            ))}
          </div>
        ))}

      {tab === "watched" &&
        (watched.length === 0 ? (
          <div className="empty">{t("discover.watchedEmpty")}</div>
        ) : fWatched.length === 0 ? (
          <div className="empty">{t("discover.searchNoResults")}</div>
        ) : (
          <div className="listas-grid">
            {fWatched.map((s) => (
              <WatchedRow
                key={s.id}
                series={s}
                genres={genresMap[s.id]}
                onChanged={load}
                onOpenSeries={onOpenSeries}
              />
            ))}
          </div>
        ))}
    </>
  );
}
