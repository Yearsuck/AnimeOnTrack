import { useEffect, useMemo, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import {
  getGenreCards,
  getTypeStats,
  getWatchSummary,
  getWatchInsights,
  getPopularityBias,
  getStatsGraph,
  backfillGenres,
  listLibrary,
} from "../api";
import { useT } from "../i18n";
import type { TypeStat, WatchSummary, WatchInsights, PopularityBias, SeriesGraphNode, GenreCard, LibraryItem } from "../types";
import { StatsGraph } from "./StatsGraph";
import { StatsRings } from "./StatsRings";
import { StatsInsights } from "./StatsInsights";
import "../styles/stats.css";

type StatsView = "resumen" | "grafo" | "generos";

export function Stats({
  active,
  onOpenSeries,
}: {
  active: boolean;
  onOpenSeries?: (seriesId: number) => void;
}) {
  const t = useT();
  const [summary, setSummary] = useState<WatchSummary | null>(null);
  const [insights, setInsights] = useState<WatchInsights | null>(null);
  const [popularityBias, setPopularityBias] = useState<PopularityBias | null>(null);
  const [genres, setGenres] = useState<GenreCard[]>([]);
  const [types, setTypes] = useState<TypeStat[]>([]);
  const [graph, setGraph] = useState<SeriesGraphNode[]>([]);
  const [library, setLibrary] = useState<LibraryItem[]>([]);
  const [view, setView] = useState<StatsView>("resumen");
  const [backfilling, setBackfilling] = useState(false);
  const [loaded, setLoaded] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const watchedMap = useMemo(() => {
    const map = new Map<number, number>();
    for (const item of library) {
      const eps =
        item.seen_episodes > 0
          ? item.seen_episodes
          : item.watched_externally
          ? Math.max(item.total_episodes, 1)
          : 0;
      map.set(item.series.id, eps);
    }
    return map;
  }, [library]);
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
      const [s, i, p, g, t, gr, lib] = await Promise.all([
        getWatchSummary(),
        getWatchInsights(),
        getPopularityBias(),
        getGenreCards(),
        getTypeStats(),
        getStatsGraph(),
        listLibrary().catch(() => []),
      ]);
      if (seq !== loadSeq.current) return;
      setSummary(s);
      setInsights(i);
      setPopularityBias(p);
      setGenres(g);
      setTypes(t);
      setGraph(gr);
      setLibrary(lib ?? []);
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
    <div className="page stats-page">
      <div className="page-head">
        <h2 className="page-title">{t("nav.stats")}</h2>
        <div className="tabs" role="tablist">
          <button
            type="button"
            role="tab"
            aria-selected={view === "resumen"}
            className={`tab ${view === "resumen" ? "active" : ""}`}
            onClick={() => setView("resumen")}
          >
            {t("stats.insightsHeading")}
          </button>
          <button
            type="button"
            role="tab"
            aria-selected={view === "grafo"}
            className={`tab ${view === "grafo" ? "active" : ""}`}
            onClick={() => setView("grafo")}
          >
            {t("stats.tabGraph")}
          </button>
          <button
            type="button"
            role="tab"
            aria-selected={view === "generos"}
            className={`tab ${view === "generos" ? "active" : ""}`}
            onClick={() => setView("generos")}
          >
            {t("stats.byGenre")}
          </button>
        </div>
        <div className="spacer" />
        <button className="btn btn-ghost" onClick={runBackfill} disabled={backfilling}>
          {backfilling ? t("stats.backfilling") : t("stats.backfillButton")}
        </button>
      </div>

      {error && (
        <div className="notice notice-danger" role="alert">
          <p>{t("stats.loadError", { msg: error })}</p>
          <button className="btn" onClick={load}>
            {t("stats.retry")}
          </button>
        </div>
      )}

      {!loaded && !error && (
        <div className="kpi-strip" role="status" aria-label={t("stats.loading")}>
          {[0, 1, 2, 3].map((i) => (
            <div className="kpi-card" key={i}>
              <div className="skeleton skeleton-kpi-label" />
              <div className="skeleton skeleton-kpi-val" />
              <div className="skeleton skeleton-kpi-sub" />
            </div>
          ))}
        </div>
      )}

      {loaded && (
        <>
          <div className={view === "resumen" ? "tab-panel" : "tab-panel hidden"}>
            {summary && insights && (
              <StatsInsights insights={insights} summary={summary} popularityBias={popularityBias} />
            )}
          </div>

          <div className={view === "grafo" ? "tab-panel" : "tab-panel hidden"}>
            <div className="series-block">
              <StatsGraph
                seriesList={graph}
                active={active && view === "grafo"}
                onOpenSeries={onOpenSeries}
                watchedEpisodes={watchedMap}
              />
            </div>
          </div>

          <div className={view === "generos" ? "tab-panel" : "tab-panel hidden"}>
            <StatsRings genres={genres} types={types} />
          </div>
        </>
      )}
    </div>
  );
}
