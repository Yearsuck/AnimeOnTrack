import { useEffect, useMemo, useState } from "react";
import { listAiringSeason, setFollowed } from "../api";
import { useT } from "../i18n";
import type { AiringItem, Series } from "../types";
import { AiringSpotlight } from "./AiringSpotlight";

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
function isWithinLastThreeMonths(firstEpisodeAt: number): boolean {
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

type AiringViewMode = "all" | "season" | "week";

export function AiringGrid({
  onOpenSeries,
  refreshSignal,
}: {
  onOpenSeries: (s: Series) => void;
  refreshSignal?: number;
}) {
  const t = useT();
  const [items, setItems] = useState<AiringItem[]>([]);
  const [query, setQuery] = useState("");
  const [onlyFollowed, setOnlyFollowed] = useState(false);
  const [viewMode, setViewMode] = useState<AiringViewMode>("all");

  async function load() {
    setItems(await listAiringSeason());
  }
  useEffect(() => {
    load();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [refreshSignal]);

  async function toggle(e: React.MouseEvent, s: Series) {
    e.stopPropagation();
    await setFollowed(s.id, !s.followed);
    await load();
  }

  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase();
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
  }, [items, query, onlyFollowed, viewMode]);

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
          className={`btn ${onlyFollowed ? "btn-success" : "btn-ghost"}`}
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
          {viewMode === "week" ? t("airing.weekEmpty") : t("common.noResults")}
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
                      <div key={s.id} className="card" onClick={() => onOpenSeries(s)}>
                        <div className="poster">
                          {s.followed && <span className="chip">{t("airing.followingChip")}</span>}
                          {countdownLabel(s.next_episode_at, t) && (
                            <span className="chip chip-countdown">{countdownLabel(s.next_episode_at, t)}</span>
                          )}
                          {s.cover_url ? (
                            <img src={s.cover_url} alt={s.title} loading="lazy" />
                          ) : null}
                        </div>
                        <div className="card-body">
                          <div className="card-title">{s.title}</div>
                          <button
                            className={`btn follow-btn ${s.followed ? "on" : ""}`}
                            onClick={(e) => toggle(e, s)}
                          >
                            {s.followed ? t("airing.followingBtn") : t("airing.followBtn")}
                          </button>
                        </div>
                      </div>
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
            <div key={s.id} className="card" onClick={() => onOpenSeries(s)}>
              <div className="poster">
                {s.followed && <span className="chip">{t("airing.followingChip")}</span>}
                {countdownLabel(s.next_episode_at, t) && (
                  <span className="chip chip-countdown">{countdownLabel(s.next_episode_at, t)}</span>
                )}
                {s.cover_url ? (
                  <img src={s.cover_url} alt={s.title} loading="lazy" />
                ) : null}
              </div>
              <div className="card-body">
                <div className="card-title">{s.title}</div>
                <button
                  className={`btn follow-btn ${s.followed ? "on" : ""}`}
                  onClick={(e) => toggle(e, s)}
                >
                  {s.followed ? t("airing.followingBtn") : t("airing.followBtn")}
                </button>
              </div>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
