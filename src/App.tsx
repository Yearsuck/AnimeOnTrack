import { Suspense, lazy, useCallback, useEffect, useRef, useState } from "react";
import { BackLevel, useBackLayer } from "./lib/backStack";
import { Onboarding } from "./views/Onboarding";
import { AiringGrid } from "./views/AiringGrid";
import { Pending } from "./views/Pending";
import { ProgressBar } from "./views/ProgressBar";
import { BusyLabel } from "./components/BusyLabel";
import { WindowControls } from "./views/WindowControls";
import { ShortcutsOverlay } from "./views/ShortcutsOverlay";
import { UpdateBanner } from "./views/UpdateBanner";
import { Tutorial } from "./views/Tutorial";
import { LaunchHost } from "./components/LaunchOverlay";
import {
  TUTORIAL_AFTER_SPLASH_MS,
  readTutorialSeen,
  shouldAutoShowTutorial,
  writeTutorialSeen,
} from "./lib/tutorial";
import { CatalogSectionSwitch } from "./views/CatalogSectionSwitch";
import {
  CATALOG_SECTIONS,
  DEFAULT_CATALOG_SECTION,
  discoverSubViewFor,
  type CatalogSection,
} from "./lib/catalogSection";
import { installSlidingIndicators, tabDirection } from "./lib/slidingIndicator";
import { installInteractionTracker, posterOfLastInteraction, transitionTo } from "./lib/viewTransition";
import { pop } from "./lib/motion";
import { shortcutFor, shouldNavigateToCatalogOnSearch } from "./lib/shortcuts";
import {
  listAiring,
  refresh,
  rescanAiring,
  pendingCount,
  maybeSyncCatalogIncremental,
  linkSeriesToCatalog,
  linkUnlinkedViaAnilist,
  backfillCatalogMetadata,
  refreshCatalogStatus,
  listLibrary,
} from "./api";
import { notifyNewEpisodes, readNotifyPref, shouldNotify } from "./lib/notify";
import { useT } from "./i18n";
import type { Series } from "./types";
import { isMobile } from "./lib/platform";

const loadLibrary = () => import("./views/Library");
const Library = lazy(() => loadLibrary().then((m) => ({ default: m.Library })));

const loadDescubrir = () => import("./views/Descubrir");
const Descubrir = lazy(() => loadDescubrir().then((m) => ({ default: m.Descubrir })));

const loadCatalog = () => import("./views/Catalog");
const Catalog = lazy(() => loadCatalog().then((m) => ({ default: m.Catalog })));

// Lazy-loaded to defer heavy 3D graph dependencies (three, react-force-graph-3d)
// until Estadísticas is first visited.
const loadStats = () => import("./views/Stats");
const Stats = lazy(() => loadStats().then((m) => ({ default: m.Stats })));

const loadSettings = () => import("./views/Settings");
const Settings = lazy(() => loadSettings().then((m) => ({ default: m.Settings })));

// Once the module has loaded, render the component directly: React.lazy would
// still suspend on its first render inside the view transition's flushSync.
let loadedSeriesDetail: (typeof import("./views/SeriesDetail"))["SeriesDetail"] | null = null;
const loadSeriesDetail = () =>
  import("./views/SeriesDetail").then((m) => {
    loadedSeriesDetail = m.SeriesDetail;
    return m;
  });
const SeriesDetail = lazy(() => loadSeriesDetail().then((m) => ({ default: m.SeriesDetail })));

const TAB_ORDER: readonly string[] = ["pending", "airing", "library", "catalog", "stats", "settings"];

type View =
  | "loading"
  | "onboarding"
  | "pending"
  | "airing"
  | "library"
  | "stats"
  | "catalog"
  | "settings";

/**
 * Fire-and-forget catalog maintenance, none of which blocks the UI.
 * Deliberately sequential, not Promise.all: the two AniList-facing
 * steps are each paced at ~28.6 req/min against a 30/min cap, so
 * overlapping them would just earn 429s.
 * Also called right after Onboarding: a brand-new install never reaches the startup call
 * (listAiring throws first), so without this the catalog would not start syncing until the
 * next launch and "Esta temporada" would stay empty.
 */
async function runCatalogMaintenance() {
  // Backfills anilist_catalog.start_date so "Esta temporada" can
  // resolve unfollowed airing shows too. Throttled server-side.
  await maybeSyncCatalogIncremental().catch(() => {});
  // Refills the columns that predate the first full catalog crawl
  // (duration, romaji, studio...). Self-throttling: once no rows are
  // stale it returns immediately, and an interrupted run simply picks
  // up where it left off on the next launch.
  await backfillCatalogMetadata().catch(() => {});
  // Local and instant, but it needs the backfilled romaji titles to
  // match well, so it goes right after the backfill.
  await linkSeriesToCatalog().catch(() => {});
  // Paced AniList refresh of airing/recent statuses (~20s of
  // requests, once per 45 min): last so it never delays the linking.
  await refreshCatalogStatus().catch(() => {});
  // Last: ~1 request per unlinked title, so it never delays the steps above.
  await linkUnlinkedViaAnilist().catch(() => {});
}

export default function App() {
  const t = useT();
  const [view, setView] = useState<View>("loading");
  // A series' detail page is an overlay on top of the current tab, not a
  // tab of its own — see the render below. Opening/closing it never
  // touches `view`, so whichever tab was showing underneath is never
  // unmounted and its scroll position needs no restoring at all: it was
  // simply never disturbed.
  const [selected, setSelected] = useState<Series | null>(null);
  const DetailView = loadedSeriesDetail ?? SeriesDetail;
  const selectedRef = useRef(selected);
  selectedRef.current = selected;
  // The poster the detail was opened from, so closing can morph the cover back.
  const openerRef = useRef<Element | null>(null);
  // Warm the detail chunk on the press that will open it.
  useEffect(() => installInteractionTracker(() => void loadSeriesDetail().catch(() => {})), []);
  // Open/close go through a view transition (poster <-> detail cover morph);
  // without support or with reduced motion it is a plain state change.
  const openSeries = useCallback(async (s: Series, opts?: { morph?: boolean }) => {
    // Opening a related series from inside a detail: no card to morph from.
    const from = selectedRef.current || opts?.morph === false ? null : posterOfLastInteraction();
    // The detail must be renderable synchronously inside the transition, or the
    // morph would have no target: wait for the chunk first (instant once cached).
    if (!loadedSeriesDetail) await loadSeriesDetail().catch(() => {});
    openerRef.current = from;
    transitionTo(() => setSelected(s), { from, opens: Boolean(loadedSeriesDetail) });
  }, []);
  const closingRef = useRef(false);
  const closeSeries = useCallback(() => {
    // A second Escape/click while the first close is in flight is ignored.
    if (!selectedRef.current || closingRef.current) return;
    closingRef.current = true;
    const opener = openerRef.current;
    transitionTo(() => setSelected(null), {
      findTo: () => (opener?.isConnected ? opener : null),
      after: () => {
        openerRef.current = null;
        closingRef.current = false;
      },
    });
  }, []);
  const [pending, setPending] = useState(0);
  // Pendientes badge: a quick pop when the count changes (not on the first load).
  const badgeRef = useRef<HTMLSpanElement | null>(null);
  const prevPending = useRef(0);
  // Set by refreshBadge: false only for the very first count load (the startup
  // count landing is not something to celebrate, even when it is 0 -> N later).
  const popOk = useRef(false);
  const countLoaded = useRef(false);
  useEffect(() => {
    if (prevPending.current === pending) return;
    prevPending.current = pending;
    if (popOk.current) pop(badgeRef.current);
  }, [pending]);
  // Short window after a tab switch / first load in which grids stagger their
  // first cards in (CSS: [data-fresh]); and after a Catálogo section switch the
  // direction its content slides from (CSS: [data-sdir]).
  const [fresh, setFresh] = useState(true);
  const [sectionDir, setSectionDir] = useState<"fwd" | "back" | null>(null);
  const [refreshing, setRefreshing] = useState(false);
  const [airingRefreshSignal, setAiringRefreshSignal] = useState(0);
  // Pendientes' equivalent of `airingRefreshSignal`: bumping it makes the
  // mounted Pending view re-fetch its episode list. The badge alone is not
  // enough — `refreshBadge` only re-counts, so without this the list under
  // the badge kept showing the pre-refresh episodes (stale after startup's
  // background refresh(), after the topbar "Actualizar", and after the
  // SeriesDetail overlay marked episodes seen on top of it).
  const [pendingRefreshSignal, setPendingRefreshSignal] = useState(0);
  // Estadísticas mounts the three.js/d3-force graph, which is expensive to
  // build and re-layout — see docs/superpowers/specs/2026-07-10-stats-graph-cache-design.md.
  // Once visited it stays mounted (hidden via CSS, not unmounted) so
  // switching tabs and back doesn't rebuild the renderer or re-scramble the
  // layout. Lazy: only mounts on first visit, not eagerly at app start.
  const [statsVisited, setStatsVisited] = useState(false);
  // Catálogo's active sub-section (Explorar / Descubrir / Listas). Kept here,
  // not in the view, so it survives switching tabs and back.
  const [catalogSection, setCatalogSection] = useState<CatalogSection>(DEFAULT_CATALOG_SECTION);
  const [shortcutsOpen, setShortcutsOpen] = useState(false);
  const [tutorialOpen, setTutorialOpen] = useState(false);
  const navigateRef = useRef(navigate);
  navigateRef.current = navigate;
  const closeTutorial = useCallback(() => {
    writeTutorialSeen();
    setTutorialOpen(false);
    // Phone: land on Pendientes after the tour (the device test saw a blank screen there).
    if (isMobile) navigateRef.current("pending");
  }, []);

  const refreshBadge = useCallback(async () => {
    try {
      const n = await pendingCount();
      popOk.current = countLoaded.current;
      countLoaded.current = true;
      setPending(n);
    } catch {
      /* no source yet */
    }
  }, []);

  // The `onChanged` every view should get: the badge and the Pendientes list
  // are two views of the same data, so they always move together. Passing
  // plain `refreshBadge` anywhere that can change the pending set is what let
  // the two drift apart.
  const onPendingDataChanged = useCallback(async () => {
    await refreshBadge();
    setPendingRefreshSignal((n) => n + 1);
  }, [refreshBadge]);

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
        maybeAutoShowTutorial();
        setRefreshing(true);
        const n = await refresh().catch(() => 0);
        setRefreshing(false);
        maybeNotify(n);
        // Pending is already mounted by now (it's the first screen), so it has
        // fetched the *pre*-refresh list — reload it, not just the badge.
        await onPendingDataChanged();
        void runCatalogMaintenance();
      } catch {
        setView("onboarding");
      }
    })();
  }, [onPendingDataChanged]);

  // Keep AniList catalog status fresh while the app is open: re-check every
  // 30 minutes and whenever the window regains visibility. Both rely on the
  // server-side 45-minute throttle and swallow errors.
  useEffect(() => {
    const trigger = () => {
      // New dates only show up if the airing grid re-reads them afterwards.
      void refreshCatalogStatus()
        .then((n) => {
          if (n != null) setAiringRefreshSignal((c) => c + 1);
        })
        .catch(() => {});
    };
    const interval = setInterval(trigger, 30 * 60 * 1000);
    const onVisibilityChange = () => {
      if (document.visibilityState === "visible") {
        trigger();
      }
    };
    document.addEventListener("visibilitychange", onVisibilityChange);
    return () => {
      clearInterval(interval);
      document.removeEventListener("visibilitychange", onVisibilityChange);
    };
  }, []);

  // Everyone who has not seen the tutorial gets it once, existing users
  // updating from a build without it included.
  function maybeAutoShowTutorial() {
    if (!shouldAutoShowTutorial({ seen: readTutorialSeen() })) return;
    // Wait for the startup splash overlay to go away.
    setTimeout(() => setTutorialOpen(true), TUTORIAL_AFTER_SPLASH_MS);
  }

  function maybeNotify(n: number) {
    if (
      shouldNotify({
        newCount: n,
        enabled: readNotifyPref(),
        windowFocused: document.hasFocus(),
      })
    ) {
      void notifyNewEpisodes(n, t("notify.title"), t("notify.body", { count: n }));
    }
  }

  async function doRefresh() {
    setRefreshing(true);
    try {
      // On the airing tab, also rescan the catalog for new/changed series,
      // not just episodes of series you already follow.
      if (view === "airing") {
        await rescanAiring().catch(() => 0);
        setAiringRefreshSignal((n) => n + 1);
      }
      const n = await refresh();
      maybeNotify(n);
    } finally {
      setRefreshing(false);
      // Reloads Pendientes as well as its badge — this is the "Actualizar"
      // button, and on the Pendientes tab the whole point is seeing the new
      // episodes appear, not just the count next to the tab change.
      await onPendingDataChanged();
      // On the airing tab, bump the signal once more after refresh() so the
      // grid picks up covers that were downloaded during this refresh cycle
      // (the pre-refresh bump above only gets the airing-listing update;
      // covers are written during refresh(), which runs after that).
      if (view === "airing") setAiringRefreshSignal((n) => n + 1);
      if (view !== "airing") navigate("pending");
    }
  }

  useEffect(() => {
    if (view === "stats") setStatsVisited(true);
  }, [view]);

  // Every tab besides Stats fully unmounts on switch (see the render below),
  // which used to reset its scroll to the top — annoying on long lists
  // (Library, Catalog) where switching away and back meant scrolling all the
  // way back down. Remember each tab's own scroll offset (inside its
  // .view-panel, which now owns its own scroll — see the app-shell layout
  // in styles.css) and restore it.
  const scrollPositions = useRef<Partial<Record<View, number>>>({});
  const panelRef = useRef<HTMLDivElement | null>(null);
  // Which way the incoming tab slides in from (see .view-enter in styles.css).
  const [navDir, setNavDir] = useState<"fwd" | "back">("fwd");
  // Sliding pill behind the nav tabs and every .seg control (no per-caller code).
  useEffect(() => installSlidingIndicators(), []);
  useEffect(() => {
    setFresh(true);
    const id = setTimeout(() => setFresh(false), 1200);
    return () => clearTimeout(id);
  }, [view]);
  const sectionTimer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  useEffect(() => () => clearTimeout(sectionTimer.current), []);
  // Section switch inside Catálogo: remember which way it moved so the new
  // content can slide in from that side; cleared once the entrance is over.
  function changeSection(next: CatalogSection) {
    if (next === catalogSection) return;
    clearTimeout(sectionTimer.current);
    setSectionDir(tabDirection(CATALOG_SECTIONS, catalogSection, next));
    setCatalogSection(next);
    sectionTimer.current = setTimeout(() => setSectionDir(null), 500);
  }
  function navigate(next: View) {
    if (panelRef.current) scrollPositions.current[view] = panelRef.current.scrollTop;
    setNavDir(tabDirection(TAB_ORDER, view, next));
    openerRef.current = null;
    clearTimeout(sectionTimer.current);
    setSectionDir(null);
    setSelected(null);
    setView(next);
  }
  useEffect(() => {
    const target = scrollPositions.current[view];
    if (target === undefined) return;
    // The incoming tab's data (Pending/Library/Catalog...) often fetches and
    // renders after this effect first fires, growing the panel past what it
    // was on the previous paint — a one-shot scroll here would just get
    // overwritten once the real rows land. Keep nudging it back until the
    // panel's height settles, budgeted by wall-clock time rather than a
    // frame count: a Tauri IPC round-trip for a long list can easily take
    // longer than a handful of animation frames.
    const deadline = performance.now() + 5000;
    let lastHeight = -1;
    let stableFrames = 0;
    const tick = () => {
      const el = panelRef.current;
      if (!el) return;
      el.scrollTop = target;
      const h = el.scrollHeight;
      stableFrames = h === lastHeight ? stableFrames + 1 : 0;
      lastHeight = h;
      if (stableFrames < 6 && performance.now() < deadline) requestAnimationFrame(tick);
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
  // - The pending badge *and* the Pendientes list are scoped to the active
  //   source too, so both need re-fetching.
  // Landing on "airing" gives an immediate, visible confirmation that the
  // switch worked (the whole point of the live-verification requirement).
  async function onSiteChanged() {
    setStatsVisited(false);
    setAiringRefreshSignal((n) => n + 1);
    await onPendingDataChanged();
    navigate("airing");
  }

  // System back (Android) / mouse back (desktop), top first: overlays (each
  // registers its own layer), the series detail, the Descubrir section, then
  // the main view back to Pendientes. At Pendientes with nothing open no
  // history entry is left, so Android's back exits the app.
  useBackLayer(selected !== null, closeSeries);
  useBackLayer(
    view === "catalog" && catalogSection !== DEFAULT_CATALOG_SECTION,
    () => changeSection(DEFAULT_CATALOG_SECTION),
    BackLevel.tab
  );
  useBackLayer(
    view !== "pending" && view !== "loading" && view !== "onboarding",
    () => navigate("pending"),
    BackLevel.view
  );

  useEffect(() => {
    function handleKeyDown(e: KeyboardEvent) {
      const target = e.target as HTMLElement | null;
      const tag = target?.tagName?.toUpperCase();
      const targetIsEditable =
        tag === "INPUT" ||
        tag === "TEXTAREA" ||
        tag === "SELECT" ||
        Boolean(target?.isContentEditable);

      const action = shortcutFor({
        key: e.key,
        ctrlKey: e.ctrlKey,
        metaKey: e.metaKey,
        altKey: e.altKey,
        shiftKey: e.shiftKey,
        targetIsEditable,
      });

      if (!action) return;

      switch (action.kind) {
        case "navigate":
          if (view !== "loading" && view !== "onboarding") {
            navigate(action.view);
          }
          break;
        case "refresh":
          if (!refreshing) {
            void doRefresh();
          }
          break;
        case "search":
          if (!shouldNavigateToCatalogOnSearch(view, catalogSection)) {
            break;
          }
          e.preventDefault();
          setCatalogSection("explore");
          navigate("catalog");
          requestAnimationFrame(() => {
            requestAnimationFrame(() => {
              const input = document.querySelector<HTMLInputElement>("input[data-shortcut-search]");
              input?.focus();
            });
          });
          break;
        case "help":
          e.preventDefault();
          setShortcutsOpen((prev) => !prev);
          break;
        case "escape":
          if (shortcutsOpen) {
            e.preventDefault();
            setShortcutsOpen(false);
          } else if (selected) {
            e.preventDefault();
            closeSeries();
          }
          break;
      }
    }

    window.addEventListener("keydown", handleKeyDown);
    return () => {
      window.removeEventListener("keydown", handleKeyDown);
    };
  }, [view, refreshing, shortcutsOpen, selected, catalogSection, closeSeries]);

  // Frameless window: every screen needs *some* draggable strip with the
  // window controls, or the loading/onboarding views (which don't render the
  // main topbar) would leave the user unable to move or close the window.
  const MiniTitleBar = () => isMobile ? null : (
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
            void runCatalogMaintenance();
            // Same once-only rule as the startup path; replayable from Ajustes.
            if (shouldAutoShowTutorial({ seen: readTutorialSeen() })) {
              setTutorialOpen(true);
            }
          }}
        />
        <ProgressBar />
      </>
    );

  const preloadCatalogSection = (section: CatalogSection) => {
    if (section === "explore") void loadCatalog();
    else void loadDescubrir();
  };

  const goCatalogSection = (section: CatalogSection) => {
    setCatalogSection(section);
    navigate("catalog");
  };

  const preloadTab = (id: View) => {
    if (id === "library") void loadLibrary();
    else if (id === "catalog") preloadCatalogSection(catalogSection);
    else if (id === "stats") void loadStats();
    else if (id === "settings") void loadSettings();
  };

  // A plain render helper, not a component: a component type declared in the
  // render body is new every render and would remount the tabs (and the badge,
  // whose pop animation lives on the element).
  const tabIcons: Record<string, React.ReactNode> = {
    pending: (
      <svg className="tab-icon" width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
        <circle cx="12" cy="12" r="10" />
        <polyline points="12 6 12 12 16 14" />
      </svg>
    ),
    airing: (
      <svg className="tab-icon" width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
        <rect x="2" y="7" width="20" height="15" rx="2" ry="2" />
        <polyline points="17 2 12 7 7 2" />
      </svg>
    ),
    library: (
      <svg className="tab-icon" width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
        <path d="M4 19.5A2.5 2.5 0 0 1 6.5 17H20" />
        <path d="M6.5 2H20v20H6.5A2.5 2.5 0 0 1 4 19.5v-15A2.5 2.5 0 0 1 6.5 2z" />
      </svg>
    ),
    catalog: (
      <svg className="tab-icon" width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
        <circle cx="12" cy="12" r="10" />
        <polygon points="16.24 7.76 14.12 14.12 7.76 16.24 9.88 9.88 16.24 7.76" />
      </svg>
    ),
    stats: (
      <svg className="tab-icon" width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
        <line x1="18" y1="20" x2="18" y2="10" />
        <line x1="12" y1="20" x2="12" y2="4" />
        <line x1="6" y1="20" x2="6" y2="14" />
      </svg>
    ),
    settings: (
      <svg className="tab-icon" width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
        <circle cx="12" cy="12" r="3" />
        <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 0 1 0 2.83 2 2 0 0 1-2.83 0l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-2 2 2 2 0 0 1-2-2v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 0 1-2.83 0 2 2 0 0 1 0-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1-2-2 2 2 0 0 1 2-2h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 0 1 0-2.83 2 2 0 0 1 2.83 0l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 2-2 2 2 0 0 1 2 2v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 0 1 2.83 0 2 2 0 0 1 0 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 2 2 2 2 0 0 1-2 2h-.09a1.65 1.65 0 0 0-1.51 1z" />
      </svg>
    ),
  };

  const dragProp = isMobile ? undefined : "";

  const tab = (id: View, label: string) => (
    <button
      key={id}
      className={`tab ${view === id ? "active" : ""}`}
      data-tour={id}
      onClick={() => navigate(id)}
      onMouseEnter={() => preloadTab(id)}
      onFocus={() => preloadTab(id)}
    >
      {isMobile && tabIcons[id]}
      <span className="tab-label">{label}</span>
      {id === "pending" && pending > 0 && (
        <span className="badge" ref={badgeRef}>
          {pending}
        </span>
      )}
    </button>
  );

  return (
    <div className="app-shell">
      {/* Frameless window: the topbar IS the title bar. `data-tauri-drag-region`
          on the bar and the non-interactive areas (brand, spacer) lets the user
          drag the window; double-clicking them maximizes, same as a native bar.
          Interactive children (tabs, buttons) don't carry the attribute, so
          clicks on them never start a drag. */}
      <div className="topbar" data-tauri-drag-region={dragProp}>
        <div className="brand" data-tauri-drag-region={dragProp}>
          <img className="brand-logo" src="/app-icon.png" alt="" width={26} height={26} />
          AnimeOnTrack
        </div>
        <div className="tabs">
          {tab("pending", t("nav.pending"))}
          {tab("airing", t("nav.airing"))}
          {tab("library", isMobile ? t("nav.libraryShort") : t("nav.library"))}
          {tab("catalog", t("nav.catalog"))}
          {tab("stats", isMobile ? t("nav.statsShort") : t("nav.stats"))}
          {tab("settings", isMobile ? t("nav.settingsShort") : t("nav.settings"))}
        </div>
        <div className="spacer" data-tauri-drag-region={dragProp} />
        <button className="btn btn-primary topbar-refresh" onClick={doRefresh} disabled={refreshing} aria-busy={refreshing}>
          <BusyLabel busy={refreshing} idle={t("common.refresh")} busyText={t("common.refreshing")} />
        </button>
        <WindowControls />
      </div>
      <ProgressBar />

      <div
        className="view-stack"
        data-dir={navDir}
        data-fresh={fresh ? "" : undefined}
        data-sdir={sectionDir ?? undefined}
      >
        {view === "pending" && (
          <div className="view-panel view-enter" ref={panelRef}>
            <Pending
              onOpenSeries={openSeries}
              onChanged={onPendingDataChanged}
              refreshSignal={pendingRefreshSignal}
              onGoDiscover={() => goCatalogSection("swipe")}
            />
          </div>
        )}
        {view === "airing" && (
          <div className="view-panel view-enter" ref={panelRef}>
            <AiringGrid
              onOpenSeries={openSeries}
              refreshSignal={airingRefreshSignal}
              onChanged={onPendingDataChanged}
            />
          </div>
        )}
        {view === "library" && (
          <div className="view-panel view-enter" ref={panelRef}>
            <Suspense fallback={<div className="empty">{t("common.loading")}</div>}>
              <Library
                onOpenSeries={openSeries}
                onGoToListas={() => goCatalogSection("listas")}
              />
            </Suspense>
          </div>
        )}
        {view === "catalog" && (
          <div className="view-panel view-enter" ref={panelRef}>
            <Suspense fallback={<div className="empty">{t("common.loading")}</div>}>
              {(() => {
                const headExtra = (
                  <CatalogSectionSwitch
                    value={catalogSection}
                    onChange={changeSection}
                    onHover={preloadCatalogSection}
                  />
                );
                return catalogSection === "explore" ? (
                  <Catalog onChanged={onPendingDataChanged} headExtra={headExtra} />
                ) : (
                  <Descubrir
                    onOpenSeries={openSeries}
                    subView={discoverSubViewFor(catalogSection)}
                    onSwitchToSwipe={() => changeSection("swipe")}
                    headExtra={headExtra}
                  />
                );
              })()}
            </Suspense>
          </div>
        )}
        {statsVisited && (
          <div className="view-panel view-enter" hidden={view !== "stats"}>
            <Suspense fallback={<div className="empty">{t("common.loading")}</div>}>
              <Stats
                active={view === "stats"}
                onOpenSeries={(id) => {
                  // The graph only knows ids; SeriesDetail needs the full row.
                  listLibrary()
                    .then((items) => {
                      const hit = items.find((i) => i.series.id === id);
                      if (hit) openSeries(hit.series, { morph: false });
                    })
                    .catch((err) => console.error("open series from graph failed", err));
                }}
              />
            </Suspense>
          </div>
        )}
        {view === "settings" && (
          <div className="view-panel view-enter" ref={panelRef}>
            <Suspense fallback={<div className="empty">{t("common.loading")}</div>}>
              <Settings onSiteChanged={onSiteChanged} onReplayTutorial={() => setTutorialOpen(true)} />
            </Suspense>
          </div>
        )}

        {selected && (
          <div className="view-panel">
            {/* The detail overlay sits on top of whichever tab is showing —
                including Pendientes, whose list it can invalidate by marking
                episodes seen. `onPendingDataChanged` reloads that list
                underneath, so closing the overlay reveals the real state
                instead of the rows it just cleared. */}
            <Suspense fallback={<div className="empty">{t("common.loading")}</div>}>
              <DetailView
                series={selected}
                onBack={closeSeries}
                onChanged={onPendingDataChanged}
                onOpenSeries={openSeries}
              />
            </Suspense>
          </div>
        )}
      </div>
      <ShortcutsOverlay open={shortcutsOpen} onClose={() => setShortcutsOpen(false)} />
      <UpdateBanner />
      <LaunchHost />
      {tutorialOpen && <Tutorial onClose={closeTutorial} />}
    </div>
  );
}
