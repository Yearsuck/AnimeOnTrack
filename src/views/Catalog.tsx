import { useCallback, useEffect, useState } from "react";
import { getAnimeCatalog, openEpisode } from "../api";
import type { CatalogAnime } from "../types";

export function Catalog() {
  const [items, setItems] = useState<CatalogAnime[]>([]);
  const [page, setPage] = useState(1);
  const [hasNextPage, setHasNextPage] = useState(true);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const loadPage = useCallback(async (targetPage: number) => {
    setLoading(true);
    setError(null);
    try {
      const result = await getAnimeCatalog(targetPage);
      setItems((prev) => (targetPage === 1 ? result.items : [...prev, ...result.items]));
      setHasNextPage(result.has_next_page);
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

  return (
    <div className="page">
      <div className="page-head">
        <h2 className="page-title">Catálogo</h2>
        <span className="muted">
          Catálogo completo de anime vía AniList — {items.length} cargados
        </span>
      </div>

      {error && <div className="empty">No se pudo cargar el catálogo: {error}</div>}

      {items.length === 0 && loading ? (
        <div className="empty">Cargando…</div>
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
