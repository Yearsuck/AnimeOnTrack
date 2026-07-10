import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { getAnimeCatalog, getCatalogFacets, openEpisode, syncAnimeCatalog } from "../api";
import type { CatalogAnime, CatalogFacets, CatalogFilter, CatalogSyncProgress } from "../types";

const SCORE_OPTIONS: { value: string; label: string }[] = [
  { value: "", label: "Cualquier puntuación" },
  { value: "60", label: "60%+" },
  { value: "70", label: "70%+" },
  { value: "80", label: "80%+" },
];

const EPISODE_OPTIONS: { value: string; label: string }[] = [
  { value: "", label: "Cualquier duración" },
  { value: "1", label: "1 episodio" },
  { value: "2-12", label: "2–12 episodios" },
  { value: "13-26", label: "13–26 episodios" },
  { value: "27+", label: "27+ episodios" },
  { value: "unknown", label: "Duración desconocida" },
];

const EMPTY_FACETS: CatalogFacets = { genres: [], formats: [] };

export function Catalog() {
  const [items, setItems] = useState<CatalogAnime[]>([]);
  const [page, setPage] = useState(1);
  const [hasNextPage, setHasNextPage] = useState(true);
  const [totalSynced, setTotalSynced] = useState<number | null>(null);
  const [totalMatching, setTotalMatching] = useState<number | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [syncing, setSyncing] = useState(false);
  const [syncProgress, setSyncProgress] = useState<CatalogSyncProgress | null>(null);
  const syncingRef = useRef(false);

  const [facets, setFacets] = useState<CatalogFacets>(EMPTY_FACETS);
  const [searchInput, setSearchInput] = useState("");
  const [search, setSearch] = useState("");
  const [selectedGenres, setSelectedGenres] = useState<string[]>([]);
  const [format, setFormat] = useState("");
  const [minScore, setMinScore] = useState("");
  const [episodes, setEpisodes] = useState("");

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
    }),
    [search, selectedGenres, format, minScore, episodes]
  );

  const anyFilterActive =
    search !== "" || selectedGenres.length > 0 || format !== "" || minScore !== "" || episodes !== "";

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
        setError(String(e));
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
      setError(String(e));
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
  }

  return (
    <div className="page">
      <div className="page-head">
        <h2 className="page-title">Catálogo</h2>
        <span className="muted">
          {totalSynced !== null
            ? `${totalSynced} animes guardados en local (AniList)`
            : "Catálogo completo de anime vía AniList"}
        </span>
        <div className="spacer" />
        <button className="btn btn-primary" onClick={runSync} disabled={syncing}>
          {syncing ? "Sincronizando…" : "Sincronizar catálogo completo"}
        </button>
      </div>

      {syncing && (
        <div className="series-block" style={{ marginBottom: 20, padding: "12px 16px" }}>
          <div className="muted" style={{ fontSize: 12, marginBottom: 6 }}>
            Descargando el catálogo de AniList y guardándolo en local — puede tardar varios
            minutos (limitado a ~1 petición cada 2s para no pasarnos con el rate limit de la
            API).
          </div>
          {syncProgress && (
            <progress
              className="scanbar-track"
              value={syncProgress.synced}
              max={Math.max(syncProgress.total, syncProgress.synced, 1)}
              aria-label="Progreso de sincronización"
            />
          )}
          {syncProgress && (
            <div className="muted" style={{ fontSize: 12, marginTop: 4 }}>
              {syncProgress.synced} sincronizados
            </div>
          )}
        </div>
      )}

      {error && <div className="empty">No se pudo cargar el catálogo: {error}</div>}

      <div className="filter-bar">
        <div className="filter-row">
          <div className="search">
            <span className="icon">⌕</span>
            <input
              className="input"
              placeholder="Buscar por título…"
              value={searchInput}
              onChange={(e) => setSearchInput(e.target.value)}
            />
          </div>
          <select
            className="input"
            style={{ width: 150 }}
            value={format}
            onChange={(e) => setFormat(e.target.value)}
          >
            <option value="">Cualquier formato</option>
            {facets.formats.map((f) => (
              <option key={f} value={f}>
                {f}
              </option>
            ))}
          </select>
          <select
            className="input"
            style={{ width: 170 }}
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
            className="input"
            style={{ width: 190 }}
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
              Limpiar filtros
            </button>
          )}
          <div className="spacer" />
          {anyFilterActive && totalMatching !== null && (
            <span className="muted">{totalMatching} resultados</span>
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
        <div className="empty">Cargando…</div>
      ) : items.length === 0 && anyFilterActive ? (
        <div className="empty">Sin resultados con estos filtros.</div>
      ) : items.length === 0 ? (
        <div className="empty">
          Nada sincronizado todavía. Dale a "Sincronizar catálogo completo" para descargar el
          catálogo de AniList.
        </div>
      ) : (
        <>
          <div className="grid">
            {items.map((a) => (
              <div
                key={a.id}
                className="card"
                style={{ cursor: "pointer" }}
                onClick={() => openEpisode(a.url).catch((err) => console.error("open failed", err))}
              >
                <div className="poster">
                  {a.format && <span className="chip">{a.format}</span>}
                  {a.cover_url ? <img src={a.cover_url} alt={a.title} loading="lazy" /> : null}
                </div>
                <div className="card-body">
                  <div className="card-title">{a.title}</div>
                  <div className="muted" style={{ fontSize: 11 }}>
                    {a.genres.slice(0, 3).join(", ") || "—"}
                  </div>
                  <div className="muted" style={{ fontSize: 11, marginTop: 2 }}>
                    {a.episodes ? `${a.episodes} episodios` : "Episodios: ?"}
                    {a.average_score ? ` · ${a.average_score}%` : ""}
                  </div>
                </div>
              </div>
            ))}
          </div>

          {hasNextPage && (
            <div style={{ display: "flex", justifyContent: "center", marginTop: 24 }}>
              <button className="btn btn-primary" onClick={() => loadPage(page + 1)} disabled={loading}>
                {loading ? "Cargando…" : "Cargar más"}
              </button>
            </div>
          )}
        </>
      )}
    </div>
  );
}
