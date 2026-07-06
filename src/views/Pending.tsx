import { useEffect, useState } from "react";
import { listPending, openEpisode, setSeen } from "../api";
import type { PendingItem, Series } from "../types";

const REMOVE_MS = 220;

export function Pending({
  onOpenSeries,
  onChanged,
}: {
  onOpenSeries: (s: Series) => void;
  onChanged: () => void;
}) {
  const [items, setItems] = useState<PendingItem[]>([]);
  const [removing, setRemoving] = useState<Set<number>>(new Set());

  async function load() {
    setItems(await listPending());
  }
  useEffect(() => {
    load();
  }, []);

  async function watch(it: PendingItem) {
    await openEpisode(it.episode.url);
  }

  function markSeen(it: PendingItem) {
    setRemoving((r) => new Set(r).add(it.episode.id));
    setTimeout(async () => {
      await setSeen(it.episode.id, true);
      await load();
      onChanged();
    }, REMOVE_MS);
  }

  const groups = new Map<string, PendingItem[]>();
  for (const it of items) {
    const k = it.series.title;
    (groups.get(k) ?? groups.set(k, []).get(k)!).push(it);
  }

  return (
    <div className="page">
      <div className="page-head">
        <h2 className="page-title">Pendientes</h2>
        <span className="muted">{items.length} episodios por ver</span>
      </div>

      {items.length === 0 ? (
        <div className="empty">
          No hay episodios pendientes.
          <br />
          Sigue algún anime en “En emisión” y pulsa Actualizar.
        </div>
      ) : (
        [...groups.entries()].map(([title, eps]) => {
          const series = eps[0].series;
          return (
            <div key={title} className="series-block">
              <div className="series-head" onClick={() => onOpenSeries(series)} style={{ cursor: "pointer" }}>
                {series.cover_url && <img src={series.cover_url} alt="" />}
                <div>
                  <div className="name">{title}</div>
                  <div className="count">{eps.length} nuevo{eps.length === 1 ? "" : "s"}</div>
                </div>
              </div>
              {eps.map((it) => (
                <div
                  key={it.episode.id}
                  className={`ep-row ${removing.has(it.episode.id) ? "removing" : ""}`}
                >
                  <span className="ep-num">{it.episode.number}</span>
                  <div className="ep-main">
                    <div className="ep-title" onClick={() => watch(it)}>
                      {it.episode.title ?? `Episodio ${it.episode.number}`}
                    </div>
                    {it.episode.released_at && (
                      <div className="ep-date">{it.episode.released_at}</div>
                    )}
                  </div>
                  <div className="ep-actions">
                    <button className="btn" onClick={() => watch(it)}>
                      ▶ Ver
                    </button>
                    <button
                      className={`check ${removing.has(it.episode.id) ? "on" : ""}`}
                      title="Marcar como visto"
                      onClick={() => markSeen(it)}
                    >
                      ✓
                    </button>
                  </div>
                </div>
              ))}
            </div>
          );
        })
      )}
    </div>
  );
}
