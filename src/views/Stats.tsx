import { useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { getGenreStats, getTypeStats, getWatchSummary, getStatsGraph, backfillGenres } from "../api";
import { useT } from "../i18n";
import type { GenreStat, TypeStat, WatchSummary, SeriesGraphNode } from "../types";
import { StatsGraph } from "./StatsGraph";
import { StatsRings } from "./StatsRings";

type StatsView = "grafo" | "barras";

export function Stats({ active }: { active: boolean }) {
  const t = useT();
  const [summary, setSummary] = useState<WatchSummary | null>(null);
  const [genres, setGenres] = useState<GenreStat[]>([]);
  const [types, setTypes] = useState<TypeStat[]>([]);
  const [graph, setGraph] = useState<SeriesGraphNode[]>([]);
  const [view, setView] = useState<StatsView>("grafo");
  const [backfilling, setBackfilling] = useState(false);
  // Stats now stays mounted forever (App.tsx hides it with CSS instead of
  // unmounting, to keep the graph's three.js/d3-force state alive — see
  // docs/superpowers/specs/2026-07-10-stats-graph-cache-design.md). That
  // means it no longer refetches on every tab switch, which is correct but
  // would otherwise leave it stale after a refresh or a genre backfill
  // completes while some other tab is showing. `dirty` tracks exactly that.
  const dirtyRef = useRef(false);
  const wasActiveRef = useRef(active);

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

  // Reuse the same `refresh-progress` Tauri event ProgressBar.tsx already
  // listens to (current >= total marks the cycle complete) rather than
  // adding a new event. If Estadísticas is visible when a refresh lands,
  // reload right away; otherwise just mark dirty for the active-transition
  // effect below to pick up. No polling.
  useEffect(() => {
    const un = listen<{ current: number; total: number }>("refresh-progress", (e) => {
      if (e.payload.current < e.payload.total) return;
      if (active) {
        dirtyRef.current = false;
        load();
      } else {
        dirtyRef.current = true;
      }
    });
    return () => {
      un.then((f) => f());
    };
  }, [active]);

  // Becoming visible again after data changed while hidden.
  useEffect(() => {
    if (active && !wasActiveRef.current && dirtyRef.current) {
      dirtyRef.current = false;
      load();
    }
    wasActiveRef.current = active;
  }, [active]);

  async function runBackfill() {
    setBackfilling(true);
    try {
      await backfillGenres();
      dirtyRef.current = false;
      await load();
    } finally {
      setBackfilling(false);
    }
  }

  return (
    <div className="page">
      <div className="page-head">
        <h2 className="page-title">{t("nav.stats")}</h2>
      </div>

      {summary && (
        <div className="grid" style={{ marginBottom: 28 }}>
          <div className="card">
            <div className="card-body">
              <div className="muted" style={{ fontSize: 12 }}>{t("stats.episodesWatched")}</div>
              <div style={{ fontSize: 22, fontWeight: 700 }}>
                {summary.episodes_watched}/{summary.episodes_total}
              </div>
            </div>
          </div>
          <div className="card">
            <div className="card-body">
              <div className="muted" style={{ fontSize: 12 }}>{t("stats.distinctAnime")}</div>
              <div style={{ fontSize: 22, fontWeight: 700 }}>{summary.distinct_anime}</div>
              <div className="muted" style={{ fontSize: 11, marginTop: 2 }}>
                {t("stats.distinctAnimeHelp")}
              </div>
            </div>
          </div>
          <div className="card">
            <div className="card-body">
              <div className="muted" style={{ fontSize: 12 }}>{t("stats.followedSeries")}</div>
              <div style={{ fontSize: 22, fontWeight: 700 }}>{summary.followed_series}</div>
            </div>
          </div>
          <div className="card">
            <div className="card-body">
              <div className="muted" style={{ fontSize: 12 }}>{t("stats.backlogPending")}</div>
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
            {t("stats.tabGraph")}
          </button>
          <button
            className={`tab ${view === "barras" ? "active" : ""}`}
            onClick={() => setView("barras")}
          >
            {t("stats.tabBars")}
          </button>
        </div>
        <div className="spacer" />
        <button className="btn btn-ghost" onClick={runBackfill} disabled={backfilling}>
          {backfilling ? t("stats.backfilling") : t("stats.backfillButton")}
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
