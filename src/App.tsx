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
import { WindowControls } from "./views/WindowControls";
import {
  listAiring,
  refresh,
  rescanAiring,
  pendingCount,
  maybeSyncCatalogIncremental,
  linkSeriesToCatalog,
  backfillCatalogMetadata,
} from "./api";
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
        // Fire-and-forget catalog maintenance, none of which blocks the UI.
        // Deliberately sequential, not Promise.all: the two AniList-facing
        // steps are each paced at ~28.6 req/min against a 30/min cap, so
        // overlapping them would just earn 429s.
        (async () => {
          // Backfills anilist_catalog.start_date so "Esta temporada" can
          // resolve unfollowed airing shows too. Throttled server-side.
          await maybeSyncCatalogIncremental().catch(() => {});
          // Refills the columns that predate the first full catalog crawl
          // (duration, romaji, studio...). Self-throttling: once no rows are
          // stale it returns immediately, and an interrupted run simply picks
          // up where it left off on the next launch.
          await backfillCatalogMetadata().catch(() => {});
          // Local and instant, but it needs the backfilled romaji titles to
          // match well, so it runs last.
          await linkSeriesToCatalog().catch(() => {});
        })();
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
      if (view !== "airing") navigate("pending");
    }
  }

  useEffect(() => {
    if (view === "stats") setStatsVisited(true);
  }, [view]);

  // Every tab besides Stats fully unmounts on switch (see the render below),
  // which used to reset the page scroll to the top — annoying on long lists
  // (Library, Catalog) where switching away and back meant scrolling all the
  // way back down. Remember each view's own scroll offset and restore it.
  //
  // Captured synchronously in `navigate()` at the moment of leaving a view,
  // not via a continuous scroll listener: a listener keyed off a ref that
  // updates in its own effect races the very re-render that unmounts the
  // outgoing view (e.g. opening SeriesDetail from a long scrolled-down
  // Library list also clamps window.scrollY as the shorter detail page
  // mounts) — the ref could still read the *old* view when that clamp event
  // fired, silently overwriting the real saved offset with ~0. Reading
  // `view` from the closure at the exact call site has nothing to race.
  const scrollPositions = useRef<Partial<Record<View, number>>>({});
  function navigate(next: View) {
    scrollPositions.current[view] = window.scrollY;
    setView(next);
  }
  useEffect(() => {
    const target = scrollPositions.current[view];
    if (target === undefined) return;
    // The incoming view's data (Pending/Library/Catalog...) often fetches
    // and renders after this effect first fires, growing the page past what
    // it was on the previous paint — a one-shot scrollTo here would just get
    // overwritten once the real rows land. Keep nudging it back across a few
    // frames until the document height settles instead of assuming layout
    // is already final.
    let frame = 0;
    let lastHeight = -1;
    let stableFrames = 0;
    const tick = () => {
      window.scrollTo(0, target);
      const h = document.documentElement.scrollHeight;
      stableFrames = h === lastHeight ? stableFrames + 1 : 0;
      lastHeight = h;
      frame++;
      if (stableFrames < 3 && frame < 90) requestAnimationFrame(tick);
    };
    requestAnimationFrame(tick);
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
    navigate("airing");
  }

  function openSeries(s: Series) {
    setCameFrom(view === "detail" ? cameFrom : view);
    setSelected(s);
    navigate("detail");
  }

  // Frameless window: every screen needs *some* draggable strip with the
  // window controls, or the loading/onboarding views (which don't render the
  // main topbar) would leave the user unable to move or close the window.
  const MiniTitleBar = () => (
    <div className="titlebar-min" data-tauri-drag-region>
      <WindowControls />
    </div>
  );

  if (view === "loading")
    return (
      <>
        <MiniTitleBar />
        <div className="empty">{t("common.loading")}</div>
      </>
    );

  if (view === "onboarding")
    return (
      <>
        <MiniTitleBar />
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
      onClick={() => navigate(id)}
    >
      {label}
      {id === "pending" && pending > 0 && <span className="badge">{pending}</span>}
    </button>
  );

  return (
    <>
      {/* Frameless window: the topbar IS the title bar. `data-tauri-drag-region`
          on the bar and the non-interactive areas (brand, spacer) lets the user
          drag the window; double-clicking them maximizes, same as a native bar.
          Interactive children (tabs, buttons) don't carry the attribute, so
          clicks on them never start a drag. */}
      <div className="topbar" data-tauri-drag-region>
        <div className="brand" data-tauri-drag-region>
          <img className="brand-logo" src="/app-icon.png" alt="" width={26} height={26} />
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
        <div className="spacer" data-tauri-drag-region />
        <button className="btn btn-primary" onClick={doRefresh} disabled={refreshing}>
          {refreshing ? t("common.refreshing") : t("common.refresh")}
        </button>
        <WindowControls />
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
          onBack={() => navigate(cameFrom)}
          onChanged={refreshBadge}
        />
      )}
    </>
  );
}
