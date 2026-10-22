import { useEffect, useMemo, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { listAiringSeason, setFollowed } from "../api";
import { useT } from "../i18n";
import { initials, resolveCoverUrl, useCoverFailure } from "../lib/posterFallback";
import type { AiringItem, CatalogSyncProgress, Series } from "../types";
import { AiringSpotlight } from "./AiringSpotlight";

// A series' cover_url is a data: URI (fetched via webview), a cached local file:
// path (served via asset protocol), or an AniList CDN URL. Unfetched series still
// carry the site's raw remote thumbnail, which CSP blocks.
function AiringPoster({ title, coverUrl }: { title: string; coverUrl: string | null }) {
  const [failed, markFailed] = useCoverFailure(coverUrl);
  const src = resolveCoverUrl(coverUrl);
  if (!src || failed) {
    return (
      <div className="poster-fallback" aria-hidden="true">
        {initials(title)}
      </div>
    );
  }
  return <img src={src} alt={title} loading="lazy" onError={markFailed} />;
}

// Human label for the next-episode countdown the backend sorting is based
// on — makes the newest-first ordering legible instead of mysterious.
// Computed once per render (no ticking timer): "in 2h" / "in 3d" for a
// future release, "5h ago" for one that already aired but whose card
// hasn't rolled over yet. Null when the series carries no countdown.
export function countdownLabel(
  nextEpisodeAt: number | null,
  t: ReturnType<typeof useT>
): string | null {
  if (nextEpisodeAt == null) return null;
  const diffMs = nextEpisodeAt * 1000 - Date.now();
  const absHours = Math.abs(diffMs) / 3_600_000;
  const span =
    absHours >= 48
      ? `${Math.round(absHours / 24)} d`
      : absHours >= 1
        ? `${Math.round(absHours)} h`
        : `${Math.max(1, Math.round(Math.abs(diffMs) / 60_000))} min`;
  return diffMs >= 0 ? t("airing.countdownIn", { span }) : t("airing.countdownAgo", { span });
}

// "Esta temporada" cutoff: the series' first scraped episode aired within
// the last 3 calendar months. Computed frontend-side (see the design spec)
// so "now" is the user's own clock and a day-rollover never needs a re-query.
export function isWithinLastThreeMonths(firstEpisodeAt: number): boolean {
  const cutoff = new Date();
  cutoff.setMonth(cutoff.getMonth() - 3);
  return firstEpisodeAt * 1000 >= cutoff.getTime();
}

// Weekday grouping for "week" mode. Returns 0-6 (Sun-Sat) in user's local time.
// next_episode_at is a Unix timestamp (seconds) — multiply by 1000 for JS Date.
function getWeekday(nextEpisodeAt: number | null): number | null {
  if (nextEpisodeAt == null) return null;
  return new Date(nextEpisodeAt * 1000).getDay();
}

// Localized short weekday name (e.g. "Lun", "Mon", "Dl"), from the app's own
// i18n catalog rather than the browser's native Intl locale data — WebView2's
// bundled ICU data doesn't reliably cover every locale the app supports
// (Catalan in particular), silently falling back to the system locale
// instead of respecting the language picked in Ajustes.
const WEEKDAY_KEYS = [
  "airing.daySun",
  "airing.dayMon",
  "airing.dayTue",
  "airing.dayWed",
  "airing.dayThu",
  "airing.dayFri",
  "airing.daySat",
] as const;
function getWeekdayName(day: number, t: ReturnType<typeof useT>): string {
  return t(WEEKDAY_KEYS[day]);
}

// One poster card, shared by the grid and the week schedule. A div with
// role=button (not a <button>) because it holds the real follow <button>;
// Enter/Space open the series like Biblioteca's cards do, and the follow
// button stops its own keydown so Enter on it does not also open the detail.
function AiringCard({
  series: s,
  onOpen,
  onToggle,
}: {
  series: Series;
  onOpen: (s: Series) => void;
  onToggle: (e: React.SyntheticEvent, s: Series) => void;
}) {
  const t = useT();
  const countdown = countdownLabel(s.next_episode_at, t);
  return (
    <div
      className="card card-clickable"
      role="button"
      tabIndex={0}
      title={s.title}
      onClick={() => onOpen(s)}
      onKeyDown={(e) => {
        if (e.key === "Enter" || e.key === " ") {
          e.preventDefault();
          onOpen(s);
        }
      }}
    >
      <div className="poster">
        {s.followed && <span className="chip">{t("airing.followingChip")}</span>}
        {countdown && <span className="chip chip-countdown">{countdown}</span>}
        <AiringPoster title={s.title} coverUrl={s.cover_url} />
      </div>
      <div className="card-body">
        <div className="card-title">{s.title}</div>
        <button
          type="button"
          className={`btn follow-btn ${s.followed ? "on" : ""}`}
          aria-pressed={s.followed}
          onClick={(e) => onToggle(e, s)}
          onKeyDown={(e) => e.stopPropagation()}
        >
          {s.followed ? t("airing.followingBtn") : t("airing.followBtn")}
        </button>
      </div>
    </div>
  );
}

type AiringViewMode = "all" | "season" | "week";

export function AiringGrid({
  onOpenSeries,
  refreshSignal,
  onChanged,
}: {
  onOpenSeries: (s: Series) => void;
  refreshSignal?: number;
  onChanged?: () => void;
}) {
  const t = useT();
  const [items, setItems] = useState<AiringItem[]>([]);
  const [query, setQuery] = useState("");
  const [debouncedQuery, setDebouncedQuery] = useState("");

  // Typing is debounced so each keystroke does not re-filter every card;
  // clearing the search applies at once.
  useEffect(() => {
    if (!query) {
      setDebouncedQuery("");
      return;
    }
    const timer = setTimeout(() => {
      setDebouncedQuery(query);
    }, 150);
    return () => clearTimeout(timer);
  }, [query]);

  const [onlyFollowed, setOnlyFollowed] = useState(false);
  const [viewMode, setViewMode] = useState<AiringViewMode>("all");
  const [syncing, setSyncing] = useState(false);
  const throttleTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const syncTimeoutRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const lastRunRef = useRef<number>(0);
  const trailingPendingRef = useRef<boolean>(false);
  const loadSeqRef = useRef<number>(0);

  async function load() {
    const seq = ++loadSeqRef.current;
    const data = await listAiringSeason();
    if (seq === loadSeqRef.current) {
      setItems(data);
    }
  }

  const loadRef = useRef(load);
  loadRef.current = load;

  useEffect(() => {
    load();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [refreshSignal]);

  // Re-run load() while AniList catalog sync events arrive (throttled to at
  // most once every ~4 s with a trailing call, and once more when sync finishes)
  // so "Esta temporada" populates live during the first-run crawl.
  useEffect(() => {
    const THROTTLE_MS = 4000;
    const INACTIVITY_MS = 8000;

    const triggerThrottledLoad = () => {
      const now = Date.now();
      const elapsed = now - lastRunRef.current;
      if (elapsed >= THROTTLE_MS) {
        if (throttleTimerRef.current) {
          clearTimeout(throttleTimerRef.current);
          throttleTimerRef.current = null;
        }
        trailingPendingRef.current = false;
        lastRunRef.current = now;
        loadRef.current();
      } else {
        trailingPendingRef.current = true;
        if (!throttleTimerRef.current) {
          const wait = THROTTLE_MS - elapsed;
          throttleTimerRef.current = setTimeout(() => {
            throttleTimerRef.current = null;
            lastRunRef.current = Date.now();
            trailingPendingRef.current = false;
            loadRef.current();
          }, wait);
        }
      }
    };

    let cancelled = false;
    const un = listen<CatalogSyncProgress>("catalog-sync-progress", (e) => {
      if (cancelled) return;
      setSyncing(true);

      if (syncTimeoutRef.current) {
        clearTimeout(syncTimeoutRef.current);
      }
      syncTimeoutRef.current = setTimeout(() => {
        setSyncing(false);
        syncTimeoutRef.current = null;
      }, INACTIVITY_MS);

      // The backend has no explicit 'finished' event: its last emit carries synced === total
      // (the in-flight estimate is max(21000, synced), so this only holds once it is really over).
      const isDone = e.payload.total > 0 && e.payload.synced >= e.payload.total;

      if (isDone) {
        if (syncTimeoutRef.current) {
          clearTimeout(syncTimeoutRef.current);
          syncTimeoutRef.current = null;
        }
        setSyncing(false);

        if (throttleTimerRef.current) {
          clearTimeout(throttleTimerRef.current);
          throttleTimerRef.current = null;
        }
        trailingPendingRef.current = false;
        lastRunRef.current = Date.now();
        loadRef.current();
      } else {
        triggerThrottledLoad();
      }
    });

    return () => {
      cancelled = true;
      un.then((f) => f());
      if (throttleTimerRef.current) {
        clearTimeout(throttleTimerRef.current);
        throttleTimerRef.current = null;
      }
      if (syncTimeoutRef.current) {
        clearTimeout(syncTimeoutRef.current);
        syncTimeoutRef.current = null;
      }
    };
  }, []);

  async function toggle(e: React.SyntheticEvent, s: Series) {
    e.stopPropagation();
    await setFollowed(s.id, !s.followed);
    await load();
    onChanged?.();
  }

  const filtered = useMemo(() => {
    const q = debouncedQuery.trim().toLowerCase();
    let base = items.filter(
      (it) =>
        (!onlyFollowed || it.series.followed) &&
        (!q || it.series.title.toLowerCase().includes(q))
    );
    if (viewMode === "season") {
      base = base.filter(
        (it) => it.first_episode_at != null && isWithinLastThreeMonths(it.first_episode_at)
      );
    } else if (viewMode === "week") {
      base = base.filter((it) => it.series.next_episode_at != null);
    }
    return base;
  }, [items, debouncedQuery, onlyFollowed, viewMode]);

  const followedCount = items.filter((it) => it.series.followed).length;

  // Spotlight: in "esta temporada" the banner should only feature season
  // items, same scope as the grid below it — otherwise it contradicts the
  // filter the user just picked.
  const spotlightItems = useMemo(() => {
    if (viewMode !== "season") return items;
    return items.filter(
      (it) => it.first_episode_at != null && isWithinLastThreeMonths(it.first_episode_at)
    );
  }, [items, viewMode]);

  return (
    <div className="page">
      <div className="page-head">
        <h2 className="page-title">{t("nav.airing")}</h2>
        <div className="search">
          <span className="icon">⌕</span>
          <input
            className="input"
            placeholder={t("common.searchPlaceholder")}
            value={query}
            onChange={(e) => setQuery(e.target.value)}
          />
        </div>
        <div className="seg" role="tablist" aria-label={t("nav.airing")}>
          <button
            type="button"
            role="tab"
            aria-selected={viewMode === "all"}
            className={`seg-btn${viewMode === "all" ? " active" : ""}`}
            onClick={() => setViewMode("all")}
          >
            {t("airing.filterAll")}
          </button>
          <button
            type="button"
            role="tab"
            aria-selected={viewMode === "season"}
            className={`seg-btn${viewMode === "season" ? " active" : ""}`}
            onClick={() => setViewMode("season")}
          >
            {t("airing.filterSeason")}
          </button>
          <button
            type="button"
            role="tab"
            aria-selected={viewMode === "week"}
            className={`seg-btn${viewMode === "week" ? " active" : ""}`}
            onClick={() => setViewMode("week")}
          >
            {t("airing.filterWeek")}
          </button>
        </div>
        <button
          type="button"
          className={`btn ${onlyFollowed ? "btn-toggle-on" : "btn-ghost"}`}
          aria-pressed={onlyFollowed}
          onClick={() => setOnlyFollowed((v) => !v)}
        >
          {t("airing.followingFilter", { count: followedCount })}
        </button>
        <div className="spacer" />
        <span className="muted">{t("common.seriesCount", { count: filtered.length })}</span>
      </div>

      {viewMode === "season" && (
        <p className="muted airing-note">
          {t("airing.seasonHint")}
        </p>
      )}

      {viewMode !== "week" && (
        <AiringSpotlight items={spotlightItems} onOpenSeries={onOpenSeries} />
      )}

      {filtered.length === 0 ? (
        <div className="empty">
          {viewMode === "week"
            ? t("airing.weekEmpty")
            : viewMode === "season" && syncing
              ? t("airing.seasonSyncing")
              : t("common.noResults")}
        </div>
      ) : viewMode === "week" ? (
        <div className="week-schedule">
          {(() => {
            const today = new Date().getDay();
            const days = Array.from({ length: 7 }, (_, i) => i);
            // Order: today first, then the rest of the week
            const orderedDays = [...days.slice(today), ...days.slice(0, today)];
            return orderedDays.map((day) => {
              const dayItems = filtered.filter(
                (it) => getWeekday(it.series.next_episode_at) === day
              );
              if (dayItems.length === 0) return null;
              return (
                <div key={day} className={`week-day${day === today ? " today" : ""}`}>
                  <h3 className="week-day-header">{getWeekdayName(day, t)}</h3>
                  <div className="week-day-grid">
                    {dayItems.map(({ series: s }) => (
                      <AiringCard key={s.id} series={s} onOpen={onOpenSeries} onToggle={toggle} />
                    ))}
                  </div>
                </div>
              );
            });
          })()}
        </div>
      ) : (
        <div className="grid">
          {filtered.map(({ series: s }) => (
            <AiringCard key={s.id} series={s} onOpen={onOpenSeries} onToggle={toggle} />
          ))}
        </div>
      )}
    </div>
  );
}
