import { useCallback, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { getAnimeCatalog, openEpisode, syncAnimeCatalog } from "../api";
import type { CatalogAnime, CatalogSyncProgress } from "../types";

export function Catalog() {
  const [items, setItems] = useState<CatalogAnime[]>([]);
  const [page, setPage] = useState(1);
  const [hasNextPage, setHasNextPage] = useState(true);
  const [totalSynced, setTotalSynced] = useState<number | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [syncing, setSyncing] = useState(false);
  const [syncProgress, setSyncProgress] = useState<CatalogSyncProgress | null>(null);
  const syncingRef = useRef(false);

  const loadPage = useCallback(async (targetPage: number) => {
    setLoading(true);
    setError(null);
    try {
      const result = await getAnimeCatalog(targetPage);
      setItems((prev) => (targetPage === 1 ? result.items : [...prev, ...result.items]));
      setHasNextPage(result.has_next_page);
      setTotalSynced(result.total_synced);
      setPage(targetPage);
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    loadPage(1);
    // eslint-disable-next-line react-hooks/exhaustive-deps
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

      {items.length === 0 && loading ? (
        <div className="empty">Cargando…</div>
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
