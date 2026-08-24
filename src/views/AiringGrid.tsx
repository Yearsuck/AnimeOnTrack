import { useEffect, useMemo, useState } from "react";
import { listAiringSeason, setFollowed } from "../api";
import { useT } from "../i18n";
import type { AiringItem, Series } from "../types";
import { AiringSpotlight } from "./AiringSpotlight";

// Human label for the next-episode countdown the backend sorting is based
// on — makes the newest-first ordering legible instead of mysterious.
// Computed once per render (no ticking timer): "en 2 h" / "en 3 d" for a
// future release, "hace 5 h" for one that already aired but whose card
// hasn't rolled over yet. Null when the series carries no countdown.
function countdownLabel(nextEpisodeAt: number | null): string | null {
  if (nextEpisodeAt == null) return null;
  const diffMs = nextEpisodeAt * 1000 - Date.now();
  const absHours = Math.abs(diffMs) / 3_600_000;
  const span =
    absHours >= 48
      ? `${Math.round(absHours / 24)} d`
      : absHours >= 1
        ? `${Math.round(absHours)} h`
        : `${Math.max(1, Math.round(Math.abs(diffMs) / 60_000))} min`;
  return diffMs >= 0 ? `en ${span}` : `hace ${span}`;
}

// "Esta temporada" cutoff: the series' first scraped episode aired within
// the last 3 calendar months. Computed frontend-side (see the design spec)
// so "now" is the user's own clock and a day-rollover never needs a re-query.
function isWithinLastThreeMonths(firstEpisodeAt: number): boolean {
  const cutoff = new Date();
  cutoff.setMonth(cutoff.getMonth() - 3);
  return firstEpisodeAt * 1000 >= cutoff.getTime();
}

type SeasonFilter = "all" | "season";

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
  const [seasonFilter, setSeasonFilter] = useState<SeasonFilter>("all");

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
    return items
      .filter(
        (it) =>
          (!onlyFollowed || it.series.followed) &&
          (!q || it.series.title.toLowerCase().includes(q))
      )
      .filter(
        (it) =>
          seasonFilter === "all" ||
          (it.first_episode_at != null && isWithinLastThreeMonths(it.first_episode_at))
      );
  }, [items, query, onlyFollowed, seasonFilter]);

  const followedCount = items.filter((it) => it.series.followed).length;

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
            aria-selected={seasonFilter === "all"}
            className={`seg-btn${seasonFilter === "all" ? " active" : ""}`}
            onClick={() => setSeasonFilter("all")}
          >
            {t("airing.filterAll")}
          </button>
          <button
            type="button"
            role="tab"
            aria-selected={seasonFilter === "season"}
            className={`seg-btn${seasonFilter === "season" ? " active" : ""}`}
            onClick={() => setSeasonFilter("season")}
          >
            {t("airing.filterSeason")}
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

      {seasonFilter === "season" && (
        <p className="muted airing-note">
          {t("airing.seasonHint")}
        </p>
      )}

      <AiringSpotlight items={items} onOpenSeries={onOpenSeries} />

      {filtered.length === 0 ? (
        <div className="empty">{t("common.noResults")}</div>
      ) : (
        <div className="grid">
          {filtered.map(({ series: s }) => (
            <div key={s.id} className="card" onClick={() => onOpenSeries(s)}>
              <div className="poster">
                {s.followed && <span className="chip">{t("airing.followingChip")}</span>}
                {countdownLabel(s.next_episode_at) && (
                  <span className="chip chip-countdown">{countdownLabel(s.next_episode_at)}</span>
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
