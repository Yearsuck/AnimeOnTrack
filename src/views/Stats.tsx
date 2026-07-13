import { useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import {
  getGenreStats,
  getTypeStats,
  getWatchSummary,
  getWatchInsights,
  getStatsGraph,
  backfillGenres,
} from "../api";
import { useT } from "../i18n";
import type { GenreStat, TypeStat, WatchSummary, WatchInsights, SeriesGraphNode } from "../types";
import { StatsGraph } from "./StatsGraph";
import { StatsRings } from "./StatsRings";
import { StatsInsights } from "./StatsInsights";

type StatsView = "grafo" | "barras";

export function Stats({ active }: { active: boolean }) {
  const t = useT();
  const [summary, setSummary] = useState<WatchSummary | null>(null);
  const [insights, setInsights] = useState<WatchInsights | null>(null);
  const [genres, setGenres] = useState<GenreStat[]>([]);
  const [types, setTypes] = useState<TypeStat[]>([]);
  const [graph, setGraph] = useState<SeriesGraphNode[]>([]);
  const [view, setView] = useState<StatsView>("grafo");
  const [backfilling, setBackfilling] = useState(false);
  // Stats now stays mounted forever (App.tsx hides it with CSS instead of
  // unmounting, to keep the graph's three.js/d3-force state alive — see
  // docs/superpowers/specs/2026-07-10-stats-graph-cache-design.md). Keeping
  // it mounted preserves that three.js/d3-force state; it does NOT require
  // avoiding data refetches (refetching seriesList doesn't tear down the
  // graph — StatsGraph already diff-guards layout via its structuralKey). So
  // every false→true activation reloads unconditionally, regardless of
  // whether a refresh/backfill happened — that's the only reliable way to
  // catch follow/unfollow/seen/reclassify mutations made on other tabs,
  // none of which emit a Tauri event Stats could listen for.
  const wasActiveRef = useRef(active);

  async function load() {
    const [s, i, g, t, gr] = await Promise.all([
      getWatchSummary(),
      getWatchInsights(),
      getGenreStats(),
      getTypeStats(),
      getStatsGraph(),
    ]);
    setSummary(s);
    setInsights(i);
    setGenres(g);
    setTypes(t);
    setGraph(gr);
  }
  useEffect(() => {
    load();
  }, []);

  // Reuse the same `refresh-progress` Tauri event ProgressBar.tsx already
  // listens to (current >= total marks the cycle complete) rather than
  // adding a new event. If Estadísticas is visible when a scan finishes,
  // reload right away — this is the one case the activation reload below
  // wouldn't catch, since the user never leaves and re-enters the tab.
  useEffect(() => {
    const un = listen<{ current: number; total: number }>("refresh-progress", (e) => {
      if (e.payload.current < e.payload.total) return;
      if (active) load();
    });
    return () => {
      un.then((f) => f());
    };
  }, [active]);

  // Becoming visible again: always reload, unconditionally. Following /
  // unfollowing a series, marking episodes seen, reclassifying (want /
  // discarded / ya-vi) and cross-site follow carry-over none emit a Tauri
  // event Stats could listen for, so the only reliable moment to catch them
  // is the moment the user actually looks at this screen.
  useEffect(() => {
    if (active && !wasActiveRef.current) {
      load();
    }
    wasActiveRef.current = active;
  }, [active]);

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
        <h2 className="page-title">{t("nav.stats")}</h2>
      </div>

      {summary && (
        <div className="grid" style={{ marginBottom: 28 }}>
          <div className="card">
            <div className="card-body">
              <div className="muted" style={{ fontSize: 12 }}>{t("stats.episodesWatched")}</div>
              <div style={{ fontSize: 22, fontWeight: 700 }}>
                {summary.episodes_watched + summary.episodes_watched_external}
              </div>
              <div className="muted" style={{ fontSize: 11, marginTop: 2 }}>
                {t("stats.episodesWatchedHelp", {
                  real: summary.episodes_watched,
                  external: summary.episodes_watched_external,
                })}
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
              <div style={{ fontSize: 22, fontWeight: 700 }}>{summary.airing_followed}</div>
            </div>
          </div>
          <div className="card">
            <div className="card-body">
              <div className="muted" style={{ fontSize: 12 }}>{t("stats.backlogPending")}</div>
              <div style={{ fontSize: 22, fontWeight: 700 }}>{summary.pending_to_watch}</div>
              <div className="muted" style={{ fontSize: 11, marginTop: 2 }}>
                {t("stats.backlogPendingHelp")}
              </div>
            </div>
          </div>
          <div className="card">
            <div className="card-body">
              <div className="muted" style={{ fontSize: 12 }}>{t("stats.wishlist")}</div>
              <div style={{ fontSize: 22, fontWeight: 700 }}>{summary.backlog_want}</div>
              <div className="muted" style={{ fontSize: 11, marginTop: 2 }}>
                {t("stats.wishlistHelp")}
              </div>
            </div>
          </div>
        </div>
      )}

      {summary && insights && <StatsInsights insights={insights} summary={summary} />}

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
