import { useCallback, useEffect, useRef, useState } from "react";
import { Onboarding } from "./views/Onboarding";
import { AiringGrid } from "./views/AiringGrid";
import { Pending } from "./views/Pending";
import { SeriesDetail } from "./views/SeriesDetail";
import { Settings } from "./views/Settings";
import { Library } from "./views/Library";
import { Stats } from "./views/Stats";
import { Descubrir } from "./views/Descubrir";
import { Catalog } from "./views/Catalog";
import { ProgressBar } from "./views/ProgressBar";
import { listAiring, refresh, rescanAiring, pendingCount } from "./api";
import { useT } from "./i18n";
import type { Series } from "./types";

type View =
  | "loading"
  | "onboarding"
  | "pending"
  | "airing"
  | "library"
  | "stats"
  | "descubrir"
  | "catalog"
  | "settings"
  | "detail";

export default function App() {
  const t = useT();
  const [view, setView] = useState<View>("loading");
  const [selected, setSelected] = useState<Series | null>(null);
  const [cameFrom, setCameFrom] = useState<View>("airing");
  const [pending, setPending] = useState(0);
  const [refreshing, setRefreshing] = useState(false);
  const [airingRefreshSignal, setAiringRefreshSignal] = useState(0);
  // Estadísticas mounts the three.js/d3-force graph, which is expensive to
  // build and re-layout — see docs/superpowers/specs/2026-07-10-stats-graph-cache-design.md.
  // Once visited it stays mounted (hidden via CSS, not unmounted) so
  // switching tabs and back doesn't rebuild the renderer or re-scramble the
  // layout. Lazy: only mounts on first visit, not eagerly at app start.
  const [statsVisited, setStatsVisited] = useState(false);

  const refreshBadge = useCallback(async () => {
    try {
      setPending(await pendingCount());
    } catch {
      /* no source yet */
    }
  }, []);

  // Decide first screen: onboarding if no source yet, else pending (+ refresh-on-open).
  // Guarded against React StrictMode's dev-only double-invoke so we don't hit
  // the scraped site twice on startup.
  const startedRef = useRef(false);
  useEffect(() => {
    if (startedRef.current) return;
    startedRef.current = true;
    (async () => {
      try {
        await listAiring(); // throws if no source configured
        setView("pending");
        setRefreshing(true);
        await refresh().catch(() => 0);
        setRefreshing(false);
        await refreshBadge();
      } catch {
        setView("onboarding");
      }
    })();
  }, [refreshBadge]);

  async function doRefresh() {
    setRefreshing(true);
    try {
      // On the airing tab, also rescan the catalog for new/changed series,
      // not just episodes of series you already follow.
      if (view === "airing") {
        await rescanAiring().catch(() => 0);
        setAiringRefreshSignal((n) => n + 1);
      }
      await refresh();
    } finally {
      setRefreshing(false);
      await refreshBadge();
      if (view !== "airing") setView("pending");
    }
  }

  useEffect(() => {
    if (view === "stats") setStatsVisited(true);
  }, [view]);

  // After Settings.tsx switches the active site (and scans its airing
  // listing), every other view is now showing stale, wrong-site data:
  // - Stats stays mounted once visited (perf cache, see statsVisited's
  //   comment) so it must be reset to unmount/refetch on next visit rather
  //   than keep showing the old site's 3D graph.
  // - AiringGrid needs its refreshSignal bumped so it re-fetches even if it
  //   happens to already be mounted.
  // - The pending badge is scoped to the active source too.
  // Landing on "airing" gives an immediate, visible confirmation that the
  // switch worked (the whole point of the live-verification requirement).
  async function onSiteChanged() {
    setStatsVisited(false);
    setAiringRefreshSignal((n) => n + 1);
    await refreshBadge();
    setView("airing");
  }

  function openSeries(s: Series) {
    setCameFrom(view === "detail" ? cameFrom : view);
    setSelected(s);
    setView("detail");
  }

  if (view === "loading") return <div className="empty">{t("common.loading")}</div>;

  if (view === "onboarding")
    return (
      <>
        <Onboarding
          onDone={async () => {
            await refreshBadge();
            setView("airing");
          }}
        />
        <ProgressBar />
      </>
    );

  const Tab = ({ id, label }: { id: View; label: string }) => (
    <button
      className={`tab ${view === id ? "active" : ""}`}
      onClick={() => setView(id)}
    >
      {label}
      {id === "pending" && pending > 0 && <span className="badge">{pending}</span>}
    </button>
  );

  return (
    <>
      <div className="topbar">
        <div className="brand">
          <span className="dot" />
          AnimeOnTrack
        </div>
        <div className="tabs">
          <Tab id="pending" label={t("nav.pending")} />
          <Tab id="airing" label={t("nav.airing")} />
          <Tab id="library" label={t("nav.library")} />
          <Tab id="descubrir" label={t("nav.discover")} />
          <Tab id="catalog" label={t("nav.catalog")} />
          <Tab id="stats" label={t("nav.stats")} />
          <Tab id="settings" label={t("nav.settings")} />
        </div>
        <div className="spacer" />
        <button className="btn btn-primary" onClick={doRefresh} disabled={refreshing}>
          {refreshing ? t("common.refreshing") : t("common.refresh")}
        </button>
      </div>
      <ProgressBar />

      {view === "pending" && <Pending onOpenSeries={openSeries} onChanged={refreshBadge} />}
      {view === "airing" && (
        <AiringGrid onOpenSeries={openSeries} refreshSignal={airingRefreshSignal} />
      )}
      {view === "library" && <Library onOpenSeries={openSeries} />}
      {view === "descubrir" && <Descubrir onOpenSeries={openSeries} />}
      {view === "catalog" && <Catalog />}
      {statsVisited && (
        <div hidden={view !== "stats"}>
          <Stats active={view === "stats"} />
        </div>
      )}
      {view === "settings" && <Settings onSiteChanged={onSiteChanged} />}
      {view === "detail" && selected && (
        <SeriesDetail
          series={selected}
          onBack={() => setView(cameFrom)}
          onChanged={refreshBadge}
        />
      )}
    </>
  );
}
