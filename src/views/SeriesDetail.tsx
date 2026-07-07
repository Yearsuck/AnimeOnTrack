import { useEffect, useRef, useState } from "react";
import { listEpisodes, openEpisode, setSeenCascade } from "../api";
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
  const rowRefs = useRef<Map<number, HTMLDivElement>>(new Map());

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

  // Cascading + optimistic: update local state immediately (no reload, so
  // scroll position never jumps), then persist in the background. Marking an
  // episode seen also marks every earlier one seen; marking unseen also
  // un-marks every later one — watching stays gap-free.
  function toggleSeen(ep: Episode) {
    const n = parseInt(ep.number, 10);
    const nextSeen = !ep.seen;
    setEpisodes((prev) =>
      prev.map((e) => {
        const en = parseInt(e.number, 10);
        if (nextSeen && en <= n) return { ...e, seen: true };
        if (!nextSeen && en >= n) return { ...e, seen: false };
        return e;
      })
    );
    setSeenCascade(series.id, ep.number, nextSeen).then(onChanged);
  }

  function jumpToCurrent() {
    const firstUnseen = episodes.find((e) => !e.seen);
    const target = firstUnseen ?? episodes[episodes.length - 1];
    if (target) {
      rowRefs.current.get(target.id)?.scrollIntoView({ behavior: "smooth", block: "center" });
    }
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
        {episodes.length > 0 && (
          <button className="btn" onClick={jumpToCurrent}>
            ⇒ Ir al episodio actual
          </button>
        )}
      </div>

      {loading ? (
        <div className="empty">Cargando episodios…</div>
      ) : episodes.length === 0 ? (
        <div className="empty">Sin episodios registrados todavía. Pulsa Actualizar.</div>
      ) : (
        <div className="series-block">
          {episodes.map((ep) => (
            <div
              key={ep.id}
              ref={(el) => {
                if (el) rowRefs.current.set(ep.id, el);
                else rowRefs.current.delete(ep.id);
              }}
              className={`ep-row ${ep.seen ? "seen" : ""}`}
            >
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
