import { useEffect, useState } from "react";
import { getGenreStats, getTypeStats, getWatchSummary, getStatsGraph, backfillGenres } from "../api";
import type { GenreStat, TypeStat, WatchSummary, SeriesGraphNode } from "../types";
import { StatsGraph } from "./StatsGraph";
import { StatsRings } from "./StatsRings";

type StatsView = "grafo" | "barras";

export function Stats({ active }: { active: boolean }) {
  const [summary, setSummary] = useState<WatchSummary | null>(null);
  const [genres, setGenres] = useState<GenreStat[]>([]);
  const [types, setTypes] = useState<TypeStat[]>([]);
  const [graph, setGraph] = useState<SeriesGraphNode[]>([]);
  const [view, setView] = useState<StatsView>("grafo");
  const [backfilling, setBackfilling] = useState(false);

  async function load() {
    const [s, g, t, gr] = await Promise.all([
      getWatchSummary(),
      getGenreStats(),
      getTypeStats(),
      getStatsGraph(),
    ]);
    setSummary(s);
    setGenres(g);
    setTypes(t);
    setGraph(gr);
  }
  useEffect(() => {
    load();
  }, []);

  async function runBackfill() {
    setBackfilling(true);
    try {
      await backfillGenres();
      await load();
    } finally {
      setBackfilling(false);
    }
  }

  return (
    <div className="page">
      <div className="page-head">
        <h2 className="page-title">Estadísticas</h2>
      </div>

      {summary && (
        <div className="grid" style={{ marginBottom: 28 }}>
          <div className="card">
            <div className="card-body">
              <div className="muted" style={{ fontSize: 12 }}>Episodios vistos</div>
              <div style={{ fontSize: 22, fontWeight: 700 }}>
                {summary.episodes_watched}/{summary.episodes_total}
              </div>
            </div>
          </div>
          <div className="card">
            <div className="card-body">
              <div className="muted" style={{ fontSize: 12 }}>Series seguidas</div>
              <div style={{ fontSize: 22, fontWeight: 700 }}>{summary.followed_series}</div>
            </div>
          </div>
          <div className="card">
            <div className="card-body">
              <div className="muted" style={{ fontSize: 12 }}>Pendientes en backlog</div>
              <div style={{ fontSize: 22, fontWeight: 700 }}>{summary.backlog_want}</div>
            </div>
          </div>
        </div>
      )}

      <div style={{ display: "flex", alignItems: "center", gap: 12, marginBottom: 20 }}>
        <div className="tabs">
          <button
            className={`tab ${view === "grafo" ? "active" : ""}`}
            onClick={() => setView("grafo")}
          >
            Grafo
          </button>
          <button
            className={`tab ${view === "barras" ? "active" : ""}`}
            onClick={() => setView("barras")}
          >
            Barras
          </button>
        </div>
        <div className="spacer" />
        <button className="btn btn-ghost" onClick={runBackfill} disabled={backfilling}>
          {backfilling ? "Rellenando…" : "Rellenar géneros ahora"}
        </button>
      </div>

      {view === "grafo" ? (
        <div className="series-block">
          <StatsGraph seriesList={graph} active={active} />
        </div>
      ) : (
        <StatsRings genres={genres} types={types} />
      )}
    </div>
  );
}
