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
import { useFormatNumber } from "../lib/formatNumber";
import type { GenreStat, TypeStat, WatchSummary, WatchInsights, SeriesGraphNode } from "../types";
import { StatsGraph } from "./StatsGraph";
import { StatsRings } from "./StatsRings";
import { StatsInsights } from "./StatsInsights";

type StatsView = "grafo" | "barras";

export function Stats({ active }: { active: boolean }) {
  const t = useT();
  const n = useFormatNumber();
  const [summary, setSummary] = useState<WatchSummary | null>(null);
  const [insights, setInsights] = useState<WatchInsights | null>(null);
  const [genres, setGenres] = useState<GenreStat[]>([]);
  const [types, setTypes] = useState<TypeStat[]>([]);
  const [graph, setGraph] = useState<SeriesGraphNode[]>([]);
  const [view, setView] = useState<StatsView>("grafo");
  const [backfilling, setBackfilling] = useState(false);
  const [loaded, setLoaded] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // Monotonic id for the newest in-flight load. Three independent triggers
  // (mount, scan-finished event, tab activation) can each start one, so
  // without this the state would reflect whichever Promise.all happened to
  // settle last rather than whichever was started last.
  const loadSeq = useRef(0);
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

  // Never throws. A rejected invoke used to reject the whole Promise.all,
  // leaving every setter unfired and the tab blank forever with no hint that
  // anything had gone wrong; now it surfaces as a retryable error and any
  // previously loaded data stays on screen.
  async function load() {
    const seq = ++loadSeq.current;
    try {
      const [s, i, g, t, gr] = await Promise.all([
        getWatchSummary(),
        getWatchInsights(),
        getGenreStats(),
        getTypeStats(),
        getStatsGraph(),
      ]);
      if (seq !== loadSeq.current) return;
      setSummary(s);
      setInsights(i);
      setGenres(g);
      setTypes(t);
      setGraph(gr);
      setError(null);
    } catch (e) {
      if (seq !== loadSeq.current) return;
      setError(String(e));
    } finally {
      if (seq === loadSeq.current) setLoaded(true);
    }
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
    } catch (e) {
      setError(String(e));
    } finally {
      setBackfilling(false);
    }
  }

  return (
    <div className="page">
      <div className="page-head">
        <h2 className="page-title">{t("nav.stats")}</h2>
      </div>

      {error && (
        <div className="card" style={{ marginBottom: 20 }} role="alert">
          <div className="card-body">
            <p style={{ margin: 0 }}>{t("stats.loadError", { msg: error })}</p>
            <button className="btn btn-ghost" style={{ marginTop: 10 }} onClick={load}>
              {t("stats.retry")}
            </button>
          </div>
        </div>
      )}

      {!loaded && !error && (
        <p className="muted" role="status">
          {t("stats.loading")}
        </p>
      )}

      {summary && (
        <div className="grid" style={{ marginBottom: 28 }}>
          <div className="card">
            <div className="card-body">
              <div className="muted" style={{ fontSize: 12 }}>{t("stats.episodesWatched")}</div>
              <div style={{ fontSize: 22, fontWeight: 700 }}>
                {n(summary.episodes_watched + summary.episodes_watched_external)}
              </div>
              <div className="muted" style={{ fontSize: 11, marginTop: 2 }}>
                {t("stats.episodesWatchedHelp", {
                  real: n(summary.episodes_watched),
                  external: n(summary.episodes_watched_external),
                })}
              </div>
            </div>
          </div>
          <div className="card">
            <div className="card-body">
              <div className="muted" style={{ fontSize: 12 }}>{t("stats.distinctAnime")}</div>
              <div style={{ fontSize: 22, fontWeight: 700 }}>{n(summary.distinct_anime)}</div>
              <div className="muted" style={{ fontSize: 11, marginTop: 2 }}>
                {t("stats.distinctAnimeHelp")}
              </div>
            </div>
          </div>
          <div className="card">
            <div className="card-body">
              <div className="muted" style={{ fontSize: 12 }}>{t("stats.followedSeries")}</div>
              <div style={{ fontSize: 22, fontWeight: 700 }}>{n(summary.airing_followed)}</div>
            </div>
          </div>
          <div className="card">
            <div className="card-body">
              <div className="muted" style={{ fontSize: 12 }}>{t("stats.backlogPending")}</div>
              <div style={{ fontSize: 22, fontWeight: 700 }}>{n(summary.pending_to_watch)}</div>
              <div className="muted" style={{ fontSize: 11, marginTop: 2 }}>
                {t("stats.backlogPendingHelp")}
              </div>
            </div>
          </div>
          <div className="card">
            <div className="card-body">
              <div className="muted" style={{ fontSize: 12 }}>{t("stats.wishlist")}</div>
              <div style={{ fontSize: 22, fontWeight: 700 }}>{n(summary.backlog_want)}</div>
              <div className="muted" style={{ fontSize: 11, marginTop: 2 }}>
                {t("stats.wishlistHelp")}
              </div>
            </div>
          </div>
        </div>
      )}

      {summary && insights && <StatsInsights insights={insights} summary={summary} />}

      {/* Gated on `loaded`: `graph`/`genres`/`types` start out as empty arrays,
          and rendering them before the first load resolves flashes an
          emphatic "no followed series yet" at users who have dozens. */}
      {loaded && (
      <>
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
      </>
      )}
    </div>
  );
}
