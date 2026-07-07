import { useEffect, useMemo, useState } from "react";
import { listLibrary } from "../api";
import type { LibraryItem, Series } from "../types";

type SortMode = "name" | "recent" | "progress";

export function Library({ onOpenSeries }: { onOpenSeries: (s: Series) => void }) {
  const [items, setItems] = useState<LibraryItem[]>([]);
  const [query, setQuery] = useState("");
  const [sort, setSort] = useState<SortMode>("recent");

  async function load() {
    setItems(await listLibrary());
  }
  useEffect(() => {
    load();
  }, []);

  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase();
    const list = items.filter((it) => !q || it.series.title.toLowerCase().includes(q));
    const sorted = [...list];
    if (sort === "name") {
      sorted.sort((a, b) => a.series.title.localeCompare(b.series.title));
    } else if (sort === "recent") {
      sorted.sort((a, b) => (b.last_added ?? "").localeCompare(a.last_added ?? ""));
    } else if (sort === "progress") {
      const pct = (it: LibraryItem) => (it.total_episodes ? it.seen_episodes / it.total_episodes : 0);
      sorted.sort((a, b) => pct(a) - pct(b));
    }
    return sorted;
  }, [items, query, sort]);

  return (
    <div className="page">
      <div className="page-head">
        <h2 className="page-title">Biblioteca</h2>
        <div className="search">
          <span className="icon">⌕</span>
          <input
            className="input"
            placeholder="Buscar por nombre…"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
          />
        </div>
        <select
          className="input"
          style={{ width: 170 }}
          value={sort}
          onChange={(e) => setSort(e.target.value as SortMode)}
        >
          <option value="recent">Más reciente</option>
          <option value="name">Nombre A-Z</option>
          <option value="progress">Menos visto primero</option>
        </select>
        <div className="spacer" />
        <span className="muted">{filtered.length} series</span>
      </div>

      {filtered.length === 0 ? (
        <div className="empty">
          Aún no sigues ninguna serie.
          <br />
          Ve a “En emisión” y dale a Seguir.
        </div>
      ) : (
        <div className="series-block">
          {filtered.map((it) => {
            const pct = it.total_episodes
              ? Math.round((it.seen_episodes / it.total_episodes) * 100)
              : 0;
            return (
              <div
                key={it.series.id}
                className="lib-row"
                onClick={() => onOpenSeries(it.series)}
              >
                {it.series.cover_url && <img src={it.series.cover_url} alt="" />}
                <div className="lib-main">
                  <div className="lib-title">{it.series.title}</div>
                  <div className="progress" style={{ marginTop: 6 }}>
                    <span style={{ width: `${pct}%` }} />
                  </div>
                </div>
                <div className="lib-count">
                  {it.seen_episodes}/{it.total_episodes}
                  <span className="muted" style={{ display: "block", fontSize: 11 }}>
                    vistos
                  </span>
                </div>
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
}
