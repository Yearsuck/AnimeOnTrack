import { useEffect, useState } from "react";
import { listEpisodes, openEpisode, setSeen } from "../api";
import type { Episode, Series } from "../types";

export function SeriesDetail({
  series,
  onBack,
  onChanged,
}: {
  series: Series;
  onBack: () => void;
  onChanged: () => void;
}) {
  const [episodes, setEpisodes] = useState<Episode[]>([]);
  const [loading, setLoading] = useState(true);

  async function load() {
    setLoading(true);
    try {
      setEpisodes(await listEpisodes(series.id));
    } finally {
      setLoading(false);
    }
  }
  useEffect(() => {
    load();
  }, [series.id]);

  async function toggleSeen(ep: Episode) {
    await setSeen(ep.id, !ep.seen);
    await load();
    onChanged();
  }

  const seenCount = episodes.filter((e) => e.seen).length;
  const pct = episodes.length ? Math.round((seenCount / episodes.length) * 100) : 0;

  return (
    <div className="page">
      <button className="btn btn-ghost" onClick={onBack} style={{ marginBottom: 14 }}>
        ← Volver
      </button>

      <div className="page-head" style={{ alignItems: "flex-start" }}>
        {series.cover_url && (
          <img
            src={series.cover_url}
            alt=""
            style={{ width: 90, height: 128, objectFit: "cover", borderRadius: 10 }}
          />
        )}
        <div style={{ flex: 1 }}>
          <h2 className="page-title" style={{ marginBottom: 6 }}>
            {series.title}
          </h2>
          <a href={series.url} target="_blank" rel="noreferrer">
            Abrir página de la serie ↗
          </a>
          <div style={{ marginTop: 10 }}>
            <div className="muted" style={{ marginBottom: 4, fontSize: 12.5 }}>
              {seenCount} / {episodes.length} vistos
            </div>
            <div className="progress">
              <span style={{ width: `${pct}%` }} />
            </div>
          </div>
        </div>
      </div>

      {loading ? (
        <div className="empty">Cargando episodios…</div>
      ) : episodes.length === 0 ? (
        <div className="empty">Sin episodios registrados todavía. Pulsa Actualizar.</div>
      ) : (
        <div className="series-block">
          {episodes.map((ep) => (
            <div key={ep.id} className={`ep-row ${ep.seen ? "seen" : ""}`}>
              <span className="ep-num">{ep.number}</span>
              <div className="ep-main">
                <div className="ep-title" onClick={() => openEpisode(ep.url)}>
                  {ep.title ?? `Episodio ${ep.number}`}
                </div>
                {ep.released_at && <div className="ep-date">{ep.released_at}</div>}
              </div>
              <div className="ep-actions">
                <button className="btn" onClick={() => openEpisode(ep.url)}>
                  ▶ Ver
                </button>
                <button
                  className={`check ${ep.seen ? "on" : ""}`}
                  title={ep.seen ? "Marcar como no visto" : "Marcar como visto"}
                  onClick={() => toggleSeen(ep)}
                >
                  ✓
                </button>
              </div>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
