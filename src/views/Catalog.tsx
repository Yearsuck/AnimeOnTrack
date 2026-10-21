import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { listen } from "@tauri-apps/api/event";
import {
  decideCatalogCard,
  getAnimeCatalog,
  getCatalogFacets,
  linkCatalogSeries,
  openEpisode,
  startWatching,
  syncAnimeCatalog,
} from "../api";
import { useT } from "../i18n";
import { createFilterCache } from "../lib/createFilterCache";
import { resolveCoverUrl } from "../lib/posterFallback";
import { startSeriesSequentially, summarizeStart } from "../lib/startWatchingQueue";
import type { CatalogAnime, CatalogFacets, CatalogFilter, CatalogSyncProgress } from "../types";
import {
  DateRangeControl,
  ScoreRangeControl,
  SortSelect,
  StatusSelect,
} from "../components/filters";
import "../styles/filters.css";

// decide_catalog_card args for a catalog row. CatalogAnime.id is AniList's
// own numeric id (the catalog PK), which is exactly the anilistId the
// command keys its synthetic series row on.
function decideArgs(a: CatalogAnime, decision: "Want" | "Seen") {
  return {
    anilistId: a.id,
    title: a.title,
    url: a.url,
    posterUrl: a.cover_url,
    genres: a.genres,
    format: a.format ?? "",
    decision,
  } as const;
}

const RESTORE_PAGE_CAP = 4;

const EMPTY_FACETS: CatalogFacets = { genres: [], formats: [], studios: [] };

// Module-level cache for catalog facets so tab switching does not re-run the
// ~150ms SQLite scan across all ~22k titles on every remount.
let cachedFacets: CatalogFacets | null = null;

// Survives unmount/remount of this view (tab switch in App.tsx).
const catalogFilterCache = createFilterCache<{
  searchInput: string;
  search: string;
  selectedGenres: string[];
  format: string;
  minScore: number | null;
  maxScore: number | null;
  minStartDate: number | null;
  maxStartDate: number | null;
  status: string;
  sort: string;
  episodes: string;
  studio: string;
  hideWatched: boolean;
  secondaryOpen: boolean;
  page: number;
}>();

export function Catalog({
  onChanged,
  headExtra,
}: {
  onChanged?: () => void;
  /** Section switch (Explorar / Descubrir / Listas) rendered next to the title. */
  headExtra?: ReactNode;
}) {
  const t = useT();

  const EPISODE_OPTIONS: { value: string; label: string }[] = useMemo(
    () => [
      { value: "", label: t("catalog.anyDuration") },
      { value: "1", label: t("catalog.oneEpisode") },
      { value: "2-12", label: t("catalog.episodes2to12") },
      { value: "13-26", label: t("catalog.episodes13to26") },
      { value: "27+", label: t("catalog.episodes27plus") },
      { value: "unknown", label: t("catalog.unknownDuration") },
    ],
    [t]
  );
  const [items, setItems] = useState<CatalogAnime[]>([]);
  const [page, setPage] = useState(() => Math.min(catalogFilterCache.current?.page ?? 1, RESTORE_PAGE_CAP));
  const [hasNextPage, setHasNextPage] = useState(true);
  const [totalSynced, setTotalSynced] = useState<number | null>(null);
  const [totalMatching, setTotalMatching] = useState<number | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [syncing, setSyncing] = useState(false);
  const [syncProgress, setSyncProgress] = useState<CatalogSyncProgress | null>(null);
  const syncingRef = useRef(false);

  const [facets, setFacets] = useState<CatalogFacets>(() => cachedFacets ?? EMPTY_FACETS);
  const [searchInput, setSearchInput] = useState(() => catalogFilterCache.current?.searchInput ?? "");
  const [search, setSearch] = useState(() => catalogFilterCache.current?.search ?? "");
  const [selectedGenres, setSelectedGenres] = useState<string[]>(() => catalogFilterCache.current?.selectedGenres ?? []);
  const [format, setFormat] = useState(() => catalogFilterCache.current?.format ?? "");
  const [minScore, setMinScore] = useState<number | null>(() => catalogFilterCache.current?.minScore ?? null);
  const [maxScore, setMaxScore] = useState<number | null>(() => catalogFilterCache.current?.maxScore ?? null);
  const [minStartDate, setMinStartDate] = useState<number | null>(() => catalogFilterCache.current?.minStartDate ?? null);
  const [maxStartDate, setMaxStartDate] = useState<number | null>(() => catalogFilterCache.current?.maxStartDate ?? null);
  const [status, setStatus] = useState(() => catalogFilterCache.current?.status ?? "");
  const [sort, setSort] = useState(() => catalogFilterCache.current?.sort ?? "popularity");
  const [episodes, setEpisodes] = useState(() => catalogFilterCache.current?.episodes ?? "");
  const [studio, setStudio] = useState(() => catalogFilterCache.current?.studio ?? "");
  const [hideWatched, setHideWatched] = useState(() => catalogFilterCache.current?.hideWatched ?? false);
  const [secondaryOpen, setSecondaryOpen] = useState(() => catalogFilterCache.current?.secondaryOpen ?? false);

  // Persist filter state to the module-level cache on every change
  useEffect(() => {
    catalogFilterCache.write({
      searchInput,
      search,
      selectedGenres,
      format,
      minScore,
      maxScore,
      minStartDate,
      maxStartDate,
      status,
      sort,
      episodes,
      studio,
      hideWatched,
      secondaryOpen,
      page,
    });
  }, [
    searchInput,
    search,
    selectedGenres,
    format,
    minScore,
    maxScore,
    minStartDate,
    maxStartDate,
    status,
    sort,
    episodes,
    studio,
    hideWatched,
    secondaryOpen,
    page,
  ]);

  // Multi-select + batch actions. The selected map holds the full card object
  // (not just the id) so a selection survives pagination / scroll-out.
  const [selected, setSelected] = useState<Map<number, CatalogAnime>>(new Map());
  const [batching, setBatching] = useState(false);
  const [batchMsg, setBatchMsg] = useState<string | null>(null);

  const toggleSelect = useCallback((a: CatalogAnime) => {
    setSelected((prev) => {
      const next = new Map(prev);
      if (next.has(a.id)) next.delete(a.id);
      else next.set(a.id, a);
      return next;
    });
    setBatchMsg(null);
  }, []);

  // "Quiero ver" is fully local (decide_catalog_card never scrapes), so the
  // decides may run concurrently.
  async function batchWant() {
    setBatching(true);
    setBatchMsg(null);
    try {
      const cards = [...selected.values()];
      await Promise.all(cards.map((a) => decideCatalogCard(decideArgs(a, "Want"))));
      setSelected(new Map());
      setBatchMsg(t("catalog.batchWantDone", { count: cards.length }));
    } catch (e) {
      setBatchMsg(t("errors.generic", { detail: String(e) }));
    } finally {
      setBatching(false);
    }
  }

  // "Ya visto": the decides are local (batched), but each resulting site link
  // is a real scrape — those are run STRICTLY serialized (one at a time) so
  // marking 50 seen can never fire 50 parallel scrapes at the Cloudflare-
  // fronted site. See project-scraping-scope.
  async function batchSeen() {
    setBatching(true);
    setBatchMsg(null);
    try {
      const cards = [...selected.values()];
      const ids = await Promise.all(cards.map((a) => decideCatalogCard(decideArgs(a, "Seen"))));
      setSelected(new Map());
      for (let i = 0; i < ids.length; i++) {
        setBatchMsg(t("catalog.linking", { current: i + 1, total: ids.length }));
        try {
          await linkCatalogSeries(ids[i]);
        } catch (e) {
          console.error("linkCatalogSeries failed for", ids[i], e);
        }
      }
      setBatchMsg(t("catalog.batchSeenDone", { count: cards.length }));
    } catch (e) {
      setBatchMsg(t("errors.generic", { detail: String(e) }));
    } finally {
      setBatching(false);
    }
  }

  // "Empezar a ver": turns each catalog card into a local "Want" series and
  // requests startWatching to scrape/link on the active pirate site.
  // Must run strictly serialized via startSeriesSequentially because scraping
  // hits Cloudflare-fronted sites.
  async function runStartWatching(cards: CatalogAnime[], clearSelection = false) {
    if (cards.length === 0) return;
    setBatching(true);
    setBatchMsg(t("catalog.starting", { current: 1, total: cards.length }));
    try {
      const counts = await startSeriesSequentially(
        cards,
        async (a) => {
          try {
            const id = await decideCatalogCard(decideArgs(a, "Want"));
            const outcome = await startWatching(id);
            if (outcome.type === "Linked") return "started";
            if (outcome.type === "AlreadyLinked") return "alreadyStarted";
            return "notFound";
          } catch {
            return "failed";
          }
        },
        (current, total) => {
          setBatchMsg(t("catalog.starting", { current, total }));
        }
      );

      if (clearSelection) {
        setSelected(new Map());
      }

      // Started series show up in Pendientes: refresh its badge now, not at the next refresh.
      if (counts.started + counts.alreadyStarted > 0) onChanged?.();

      if (cards.length === 1 && counts.notFound === 1) {
        setBatchMsg(t("catalog.startNotFound", { title: cards[0].title }));
      } else {
        const { started, problems } = summarizeStart(counts);
        setBatchMsg(t("catalog.batchStartDone", { started, problems }));
      }
    } catch (e) {
      setBatchMsg(t("errors.generic", { detail: String(e) }));
    } finally {
      setBatching(false);
    }
  }

  async function batchStart() {
    await runStartWatching([...selected.values()], true);
  }

  // Debounce the raw search text into the committed value that actually
  // drives a reload — typing shouldn't fire a query per keystroke.
  useEffect(() => {
    const t = setTimeout(() => setSearch(searchInput.trim()), 300);
    return () => clearTimeout(t);
  }, [searchInput]);

  const filter = useMemo<CatalogFilter>(
    () => ({
      search: search || undefined,
      genres: selectedGenres.length ? selectedGenres : undefined,
      format: format || undefined,
      min_score: minScore !== null ? minScore : undefined,
      max_score: maxScore !== null ? maxScore : undefined,
      min_start_date: minStartDate !== null ? minStartDate : undefined,
      max_start_date: maxStartDate !== null ? maxStartDate : undefined,
      status: status || undefined,
      sort: sort || undefined,
      episodes: episodes || undefined,
      studio: studio || undefined,
      hide_watched: hideWatched || undefined,
    }),
    [
      search,
      selectedGenres,
      format,
      minScore,
      maxScore,
      minStartDate,
      maxStartDate,
      status,
      sort,
      episodes,
      studio,
      hideWatched,
    ]
  );

  const secondaryActiveCount = useMemo(
    () =>
      (minScore !== null || maxScore !== null ? 1 : 0) +
      (minStartDate !== null || maxStartDate !== null ? 1 : 0) +
      (status !== "" ? 1 : 0) +
      (sort !== "" && sort !== "popularity" ? 1 : 0) +
      (studio !== "" ? 1 : 0) +
      (episodes !== "" ? 1 : 0) +
      (hideWatched ? 1 : 0),
    [minScore, maxScore, minStartDate, maxStartDate, status, sort, studio, episodes, hideWatched]
  );

  const anyFilterActive = useMemo(
    () =>
      search !== "" ||
      selectedGenres.length > 0 ||
      format !== "" ||
      secondaryActiveCount > 0,
    [search, selectedGenres.length, format, secondaryActiveCount]
  );

  // Monotonic id for the newest in-flight load, same guard Stats.tsx uses.
  // Several triggers can overlap — a "Cargar más" click, the debounced
  // search settling, a filter select, a finished sync — and without this the
  // state reflected whichever request happened to settle last: a page-4
  // response landing after the user retyped the search got appended to the
  // now-unrelated filtered list.
  const loadSeq = useRef(0);
  // Leaving the tab must stop a load that is still walking pages: every page
  // is a DB read behind the AppState mutex and the synchronous Stats commands
  // opened next wait on that mutex on the main thread, so a quick
  // Catálogo -> Estadísticas froze the window until the old loop finished.
  useEffect(
    () => () => {
      loadSeq.current++;
    },
    []
  );

  // Replace the list with pages 1..lastPage. `lastPage === 1` is the
  // ordinary reload; larger values only come from the remount restore below.
  // Sequential rather than Promise.all: get_anime_catalog is a local SQLite
  // read behind the AppState mutex, so parallel invokes would just queue on
  // that lock.
  const loadThrough = useCallback(
    async (lastPage: number) => {
      const seq = ++loadSeq.current;
      setLoading(true);
      setError(null);
      try {
        const acc: CatalogAnime[] = [];
        let reached = 1;
        let hasNext = false;
        let synced: number | null = null;
        let matching: number | null = null;
        for (let p = 1; p <= Math.max(1, lastPage); p++) {
          const result = await getAnimeCatalog(p, filter);
          if (seq !== loadSeq.current) return;
          acc.push(...result.items);
          reached = p;
          hasNext = result.has_next_page;
          synced = result.total_synced;
          matching = result.total_matching;
          // The stored page can outrun a list that has since shrunk (a
          // narrower filter, rows removed by a sync) — stop at the real end.
          if (!result.has_next_page) break;
        }
        setItems(acc);
        setHasNextPage(hasNext);
        setTotalSynced(synced);
        setTotalMatching(matching);
        setPage(reached);
      } catch (e) {
        if (seq !== loadSeq.current) return;
        setError(t("errors.generic", { detail: String(e) }));
      } finally {
        if (seq === loadSeq.current) setLoading(false);
      }
    },
    [filter]
  );

  // "Cargar más": append the next page onto what's already shown.
  const loadMore = useCallback(async () => {
    const targetPage = page + 1;
    const seq = ++loadSeq.current;
    setLoading(true);
    setError(null);
    try {
      const result = await getAnimeCatalog(targetPage, filter);
      if (seq !== loadSeq.current) return;
      setItems((prev) => [...prev, ...result.items]);
      setHasNextPage(result.has_next_page);
      setTotalSynced(result.total_synced);
      setTotalMatching(result.total_matching);
      setPage(targetPage);
    } catch (e) {
      if (seq !== loadSeq.current) return;
      setError(t("errors.generic", { detail: String(e) }));
    } finally {
      if (seq === loadSeq.current) setLoading(false);
    }
  }, [page, filter]);

  // How far the user had paged when this view was last unmounted. Read once
  // at mount, before the persist effect above rewrites the cache.
  // Capped: rebuilding 40 pages one by one (and rendering 2000 cards) on every
  // return to the tab took seconds. "Cargar más" gets the rest back.
  const restorePageRef = useRef(Math.min(catalogFilterCache.current?.page ?? 1, RESTORE_PAGE_CAP));
  // Identity of the `filter` this effect last acted on. The mount run sees
  // the very filter it was initialized with, which is how a remount is told
  // apart from a real filter change — and it survives StrictMode's
  // dev-only double-invoke (refs persist, and the memoized filter object
  // does not change identity across it), unlike a plain "first run" flag.
  const lastFilterRef = useRef(filter);

  // Any filter change (including the debounced search settling) reloads from
  // page 1. A remount instead rebuilds pages 1..cached: reloading only page
  // 1 threw the rest of the list away while App.tsx was still spending its
  // scroll-restore budget (up to 5s of rAF ticks) chasing an offset that no
  // longer existed in a 30-row list, so the scroll silently ended up pinned
  // to the bottom.
  useEffect(() => {
    const isRemount = lastFilterRef.current === filter;
    lastFilterRef.current = filter;
    loadThrough(isRemount ? restorePageRef.current : 1);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [filter]);

  useEffect(() => {
    if (cachedFacets) {
      setFacets(cachedFacets);
      return;
    }
    getCatalogFacets()
      .then((data) => {
        cachedFacets = data;
        setFacets(data);
      })
      .catch((e) => console.error("get_catalog_facets failed", e));
  }, []);

  useEffect(() => {
    const un = listen<CatalogSyncProgress>("catalog-sync-progress", (e) => {
      setSyncProgress(e.payload);
    });
    return () => {
      un.then((f) => f());
    };
  }, []);

  const runSync = useCallback(async () => {
    if (syncingRef.current) return;
    syncingRef.current = true;
    setSyncing(true);
    setSyncProgress(null);
    try {
      await syncAnimeCatalog();
      cachedFacets = null;
      getCatalogFacets()
        .then((data) => {
          cachedFacets = data;
          setFacets(data);
        })
        .catch((e) => console.error("get_catalog_facets failed after sync", e));
      await loadThrough(1);
    } catch (e) {
      setError(t("errors.generic", { detail: String(e) }));
    } finally {
      setSyncing(false);
      syncingRef.current = false;
    }
  }, [loadThrough, t]);

  const toggleGenre = useCallback((g: string) => {
    setSelectedGenres((prev) => (prev.includes(g) ? prev.filter((x) => x !== g) : [...prev, g]));
  }, []);

  const clearFilters = useCallback(() => {
    setSearchInput("");
    setSearch("");
    setSelectedGenres([]);
    setFormat("");
    setMinScore(null);
    setMaxScore(null);
    setMinStartDate(null);
    setMaxStartDate(null);
    setStatus("");
    setSort("popularity");
    setEpisodes("");
    setStudio("");
    setHideWatched(false);
  }, []);

  const handleScoreChange = useCallback((min: number | null, max: number | null) => {
    setMinScore(min);
    setMaxScore(max);
  }, []);

  const handleDateChange = useCallback((min: number | null, max: number | null) => {
    setMinStartDate(min);
    setMaxStartDate(max);
  }, []);

  return (
    <div className="page">
      <div className="page-head">
        <h2 className="page-title">{t("nav.catalog")}</h2>
        {headExtra}
        <span className="muted">
          {totalSynced !== null
            ? t("catalog.totalSynced", { count: totalSynced })
            : t("catalog.subtitle")}
        </span>
        <div className="spacer" />
        <button className="btn btn-primary" onClick={runSync} disabled={syncing}>
          {syncing ? t("catalog.syncing") : t("catalog.syncButton")}
        </button>
      </div>

      {syncing && (
        <div className="series-block catalog-sync">
          <div className="muted catalog-sync-label">
            {t("catalog.syncInfo")}
          </div>
          {syncProgress && (
            <progress
              className="scanbar-track"
              value={syncProgress.synced}
              max={Math.max(syncProgress.total, syncProgress.synced, 1)}
              aria-label={t("catalog.syncProgressAria")}
            />
          )}
          {syncProgress && (
            <div className="muted catalog-sync-note">
              {t("catalog.syncedCount", { count: syncProgress.synced })}
            </div>
          )}
        </div>
      )}

      {error && (
        <div className="empty">
          {t("catalog.loadError")} {error}
        </div>
      )}

      <div className="filter-bar">
        <div className="filter-row">
          <div className="search">
            <span className="icon">⌕</span>
            <input
              data-shortcut-search
              className="input"
              placeholder={t("catalog.searchPlaceholder")}
              value={searchInput}
              onChange={(e) => setSearchInput(e.target.value)}
            />
          </div>
          <select
            className="input filter-select"
            value={format}
            onChange={(e) => setFormat(e.target.value)}
          >
            <option value="">{t("catalog.anyFormat")}</option>
            {facets.formats.map((f) => (
              <option key={f} value={f}>
                {f}
              </option>
            ))}
          </select>
          <button
            type="button"
            className={`btn btn-secondary filter-toggle-btn${secondaryOpen ? " active" : ""}`}
            onClick={() => setSecondaryOpen((prev) => !prev)}
            aria-expanded={secondaryOpen}
          >
            {secondaryOpen ? t("catalog.lessFilters") : t("catalog.moreFilters")}
            {secondaryActiveCount > 0 && (
              <span className="filter-active-badge">{secondaryActiveCount}</span>
            )}
          </button>
          {anyFilterActive && (
            <button className="btn btn-ghost" onClick={clearFilters}>
              {t("catalog.clearFilters")}
            </button>
          )}
          <div className="spacer" />
          {anyFilterActive && totalMatching !== null && (
            <span className="muted">{t("catalog.resultsCount", { count: totalMatching })}</span>
          )}
        </div>

        {facets.genres.length > 0 && (
          <div className="chip-row">
            {facets.genres.map((g) => (
              <button
                key={g}
                type="button"
                className={`chip-toggle${selectedGenres.includes(g) ? " active" : ""}`}
                onClick={() => toggleGenre(g)}
              >
                {g}
              </button>
            ))}
          </div>
        )}

        {secondaryOpen && (
          <div className="filter-collapsible-panel">
            <div className="filter-collapsible-row">
              <SortSelect value={sort} onChange={setSort} />
              <StatusSelect value={status} onChange={setStatus} />
              <select
                className="input filter-select"
                value={studio}
                onChange={(e) => setStudio(e.target.value)}
              >
                <option value="">{t("catalog.anyStudio")}</option>
                {facets.studios.map((s) => (
                  <option key={s} value={s}>
                    {s}
                  </option>
                ))}
              </select>
              <select
                className="input filter-select"
                value={episodes}
                onChange={(e) => setEpisodes(e.target.value)}
              >
                {EPISODE_OPTIONS.map((o) => (
                  <option key={o.value} value={o.value}>
                    {o.label}
                  </option>
                ))}
              </select>
              <button
                type="button"
                className={`chip-toggle${hideWatched ? " active" : ""}`}
                onClick={() => setHideWatched((v) => !v)}
                aria-pressed={hideWatched}
                title={t("catalog.hideWatchedTooltip")}
              >
                {hideWatched ? "✓ " : ""}{t("catalog.hideWatched")}
              </button>
            </div>

            <div className="filter-collapsible-row">
              <ScoreRangeControl
                minScore={minScore}
                maxScore={maxScore}
                onChange={handleScoreChange}
                label={t("catalog.scoreRangeLabel")}
              />
              <DateRangeControl
                minStartDate={minStartDate}
                maxStartDate={maxStartDate}
                onChange={handleDateChange}
                label={t("catalog.dateRangeLabel")}
              />
            </div>
          </div>
        )}
      </div>

      {items.length === 0 && loading ? (
        <div className="empty">{t("common.loading")}</div>
      ) : items.length === 0 && anyFilterActive ? (
        <div className="empty">{t("catalog.noResultsFiltered")}</div>
      ) : items.length === 0 ? (
        <div className="empty">{t("catalog.emptyNotSynced")}</div>
      ) : (
        <>
          {selected.size > 0 && (
            <div className="batch-bar">
              <span className="muted">{t("catalog.selectedCount", { count: selected.size })}</span>
              <div className="spacer" />
              <button className="btn btn-primary" onClick={batchStart} disabled={batching}>
                {t("catalog.batchStart")}
              </button>
              <button className="btn" onClick={batchWant} disabled={batching}>
                {t("catalog.batchWant")}
              </button>
              <button className="btn" onClick={batchSeen} disabled={batching}>
                {t("catalog.batchSeen")}
              </button>
              <button className="btn btn-ghost" onClick={() => setSelected(new Map())} disabled={batching}>
                {t("catalog.deselectAll")}
              </button>
            </div>
          )}
          {batchMsg && (
            <p className="muted catalog-count">
              {batchMsg}
            </p>
          )}
          <div className="grid">
            {items.map((a) => (
              <div
                key={a.id}
                className={`card card-selectable${selected.has(a.id) ? " selected" : ""}`}
                onClick={() => toggleSelect(a)}
              >
                <div className="poster">
                  {a.format && <span className="chip">{a.format}</span>}
                  <button
                    type="button"
                    className="card-info-btn"
                    title={t("catalog.infoTitle")}
                    aria-label={t("catalog.infoTitle")}
                    onClick={(e) => {
                      e.stopPropagation();
                      openEpisode(a.url).catch((err) => console.error("open failed", err));
                    }}
                  >
                    ℹ
                  </button>
                  {selected.has(a.id) && (
                    <span className="card-select-check" aria-hidden="true">
                      ✓
                    </span>
                  )}
                  {a.cover_url ? <img src={resolveCoverUrl(a.cover_url)!} alt={a.title} loading="lazy" /> : null}
                </div>

                <div className="card-body">
                  <div className="card-title">{a.title}</div>
                  <div className="muted card-sub">
                    {a.genres.slice(0, 3).join(", ") || "—"}
                  </div>
                  <div className="muted card-sub">
                    {a.episodes
                      ? t("catalog.episodesCount", { count: a.episodes })
                      : t("catalog.episodesUnknown")}
                    {a.average_score ? ` · ${a.average_score}%` : ""}
                  </div>
                </div>
              </div>
            ))}
          </div>

          {hasNextPage && (
            <div className="catalog-more">
              <button className="btn btn-primary" onClick={loadMore} disabled={loading}>
                {loading ? t("common.loading") : t("catalog.loadMore")}
              </button>
            </div>
          )}
        </>
      )}
    </div>
  );
}
