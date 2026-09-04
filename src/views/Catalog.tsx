import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import {
  decideCatalogCard,
  getAnimeCatalog,
  getCatalogFacets,
  linkCatalogSeries,
  openEpisode,
  syncAnimeCatalog,
} from "../api";
import { useT } from "../i18n";
import { createFilterCache } from "../lib/createFilterCache";
import type { CatalogAnime, CatalogFacets, CatalogFilter, CatalogSyncProgress } from "../types";

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

const EMPTY_FACETS: CatalogFacets = { genres: [], formats: [], studios: [] };

// Survives unmount/remount of this view (tab switch in App.tsx).
const catalogFilterCache = createFilterCache<{
  searchInput: string;
  search: string;
  selectedGenres: string[];
  format: string;
  minScore: string;
  episodes: string;
  studio: string;
  page: number;
}>();

export function Catalog() {
  const t = useT();
  const SCORE_OPTIONS: { value: string; label: string }[] = useMemo(
    () => [
      { value: "", label: t("catalog.anyScore") },
      { value: "60", label: "60%+" },
      { value: "70", label: "70%+" },
      { value: "80", label: "80%+" },
    ],
    [t]
  );

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
  const [page, setPage] = useState(() => catalogFilterCache.current?.page ?? 1);
  const [hasNextPage, setHasNextPage] = useState(true);
  const [totalSynced, setTotalSynced] = useState<number | null>(null);
  const [totalMatching, setTotalMatching] = useState<number | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [syncing, setSyncing] = useState(false);
  const [syncProgress, setSyncProgress] = useState<CatalogSyncProgress | null>(null);
  const syncingRef = useRef(false);

  const [facets, setFacets] = useState<CatalogFacets>(EMPTY_FACETS);
  const [searchInput, setSearchInput] = useState(() => catalogFilterCache.current?.searchInput ?? "");
  const [search, setSearch] = useState(() => catalogFilterCache.current?.search ?? "");
  const [selectedGenres, setSelectedGenres] = useState<string[]>(() => catalogFilterCache.current?.selectedGenres ?? []);
  const [format, setFormat] = useState(() => catalogFilterCache.current?.format ?? "");
  const [minScore, setMinScore] = useState(() => catalogFilterCache.current?.minScore ?? "");
  const [episodes, setEpisodes] = useState(() => catalogFilterCache.current?.episodes ?? "");
  const [studio, setStudio] = useState(() => catalogFilterCache.current?.studio ?? "");

  // Persist filter state to the module-level cache on every change
  useEffect(() => {
    catalogFilterCache.write({ searchInput, search, selectedGenres, format, minScore, episodes, studio, page });
  }, [searchInput, search, selectedGenres, format, minScore, episodes, studio, page]);

  // Multi-select + batch actions. The selected map holds the full card object
  // (not just the id) so a selection survives pagination / scroll-out.
  const [selected, setSelected] = useState<Map<number, CatalogAnime>>(new Map());
  const [batching, setBatching] = useState(false);
  const [batchMsg, setBatchMsg] = useState<string | null>(null);

  function toggleSelect(a: CatalogAnime) {
    setSelected((prev) => {
      const next = new Map(prev);
      if (next.has(a.id)) next.delete(a.id);
      else next.set(a.id, a);
      return next;
    });
    setBatchMsg(null);
  }

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
      min_score: minScore ? Number(minScore) : undefined,
      episodes: episodes || undefined,
      studio: studio || undefined,
    }),
    [search, selectedGenres, format, minScore, episodes, studio]
  );

  const anyFilterActive =
    search !== "" ||
    selectedGenres.length > 0 ||
    format !== "" ||
    minScore !== "" ||
    episodes !== "" ||
    studio !== "";

  const loadPage = useCallback(
    async (targetPage: number) => {
      setLoading(true);
      setError(null);
      try {
        const result = await getAnimeCatalog(targetPage, filter);
        setItems((prev) => (targetPage === 1 ? result.items : [...prev, ...result.items]));
        setHasNextPage(result.has_next_page);
        setTotalSynced(result.total_synced);
        setTotalMatching(result.total_matching);
        setPage(targetPage);
      } catch (e) {
        setError(t("errors.generic", { detail: String(e) }));
      } finally {
        setLoading(false);
      }
    },
    [filter]
  );

  // Any filter change (including the debounced search settling) reloads
  // from page 1 — this also covers the initial mount load.
  useEffect(() => {
    loadPage(1);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [filter]);

  useEffect(() => {
    getCatalogFacets()
      .then(setFacets)
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
      await loadPage(1);
    } catch (e) {
      setError(t("errors.generic", { detail: String(e) }));
    } finally {
      setSyncing(false);
      syncingRef.current = false;
    }
  }, [loadPage]);

  function toggleGenre(g: string) {
    setSelectedGenres((prev) => (prev.includes(g) ? prev.filter((x) => x !== g) : [...prev, g]));
  }

  function clearFilters() {
    setSearchInput("");
    setSearch("");
    setSelectedGenres([]);
    setFormat("");
    setMinScore("");
    setEpisodes("");
    setStudio("");
  }

  return (
    <div className="page">
      <div className="page-head">
        <h2 className="page-title">{t("nav.catalog")}</h2>
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
            value={minScore}
            onChange={(e) => setMinScore(e.target.value)}
          >
            {SCORE_OPTIONS.map((o) => (
              <option key={o.value} value={o.value}>
                {o.label}
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
                  {a.cover_url ? <img src={a.cover_url} alt={a.title} loading="lazy" /> : null}
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
              <button className="btn btn-primary" onClick={() => loadPage(page + 1)} disabled={loading}>
                {loading ? t("common.loading") : t("catalog.loadMore")}
              </button>
            </div>
          )}
        </>
      )}
    </div>
  );
}
