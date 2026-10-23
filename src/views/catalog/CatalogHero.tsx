import { useEffect, useState } from "react";
import { useT } from "../../i18n";
import { formatLabel } from "../../lib/formatLabel";
import { resolveCoverUrl } from "../../lib/posterFallback";
import type { CatalogAnime } from "../../types";
import { countdownParts, countdownShape, type CountdownParts } from "./browse";

const TICK_MS = 30_000;

/** Unix seconds now, refreshed every 30 s so the countdowns stay honest. */
export function useNowSecs(): number {
  const [now, setNow] = useState(() => Math.floor(Date.now() / 1000));
  useEffect(() => {
    const id = setInterval(() => setNow(Math.floor(Date.now() / 1000)), TICK_MS);
    return () => clearInterval(id);
  }, []);
  return now;
}

export function useCountdownText() {
  const t = useT();
  return (p: CountdownParts | null): string | null => {
    if (!p) return null;
    switch (countdownShape(p)) {
      case "days":
        return t("catalog.countdownDays", { d: p.days, h: p.hours });
      case "hours":
        return t("catalog.countdownHours", { h: p.hours, m: p.minutes });
      default:
        return t("catalog.countdownMinutes", { m: p.minutes });
    }
  };
}

/**
 * Explorar's marquee: the week's airing schedule among popular titles. The
 * featured one gets the banner, its next-episode countdown and the primary
 * actions; the rail beside it lists the rest and picking one features it.
 */
export function CatalogHero({
  items,
  busy,
  onStart,
  onWant,
  onInfo,
}: {
  items: CatalogAnime[];
  busy: boolean;
  onStart: (a: CatalogAnime) => void;
  onWant: (a: CatalogAnime) => void;
  onInfo: (a: CatalogAnime) => void;
}) {
  const t = useT();
  const now = useNowSecs();
  const countdownText = useCountdownText();
  const [pickedId, setPickedId] = useState<number | null>(null);
  const hero = items.find((a) => a.id === pickedId) ?? items[0];
  if (!hero) return null;

  const banner = hero.banner_url || hero.cover_url;
  const bannerSrc = banner ? resolveCoverUrl(banner) : null;
  const coverSrc = hero.cover_url ? resolveCoverUrl(hero.cover_url) : null;
  const left = countdownText(countdownParts(hero.next_airing_at, now));
  const facts = [
    hero.format ? formatLabel(t, hero.format) : null,
    hero.episodes ? t("catalog.episodesCount", { count: hero.episodes }) : null,
    hero.studio,
    hero.average_score ? `${hero.average_score}%` : null,
  ]
    .filter(Boolean)
    .join(" · ");

  return (
    <section className="cat-hero" aria-label={t("catalog.heroAria")}>
      {bannerSrc && <img className="cat-hero-banner" src={bannerSrc} alt="" />}
      <div className="cat-hero-scrim" aria-hidden="true" />
      <div className="cat-hero-main">
        {coverSrc && <img className="cat-hero-cover" src={coverSrc} alt="" />}
        <div className="cat-hero-text">
          <p className="cat-hero-live">
            <span className="cat-hero-dot" aria-hidden="true" />
            {t("catalog.heroAiring")}
          </p>
          <h2 className="cat-hero-title">{hero.title}</h2>
          {facts && <p className="cat-hero-facts">{facts}</p>}
          {hero.genres.length > 0 && <p className="cat-hero-genres">{hero.genres.slice(0, 4).join(", ")}</p>}
          {left && hero.next_episode != null && (
            <p className="cat-hero-countdown">
              <span className="cat-hero-countdown-label">{t("catalog.heroEpisodeIn", { ep: hero.next_episode })}</span>
              <span className="cat-hero-countdown-value">{left}</span>
            </p>
          )}
          <div className="cat-hero-actions">
            <button type="button" className="btn btn-primary" disabled={busy} onClick={() => onStart(hero)}>
              {t("catalog.batchStart")}
            </button>
            <button type="button" className="btn cat-hero-btn" disabled={busy} onClick={() => onWant(hero)}>
              {t("catalog.heroWant")}
            </button>
            <button type="button" className="btn btn-ghost cat-hero-btn-ghost" onClick={() => onInfo(hero)}>
              {t("catalog.heroInfo")}
            </button>
          </div>
        </div>
      </div>
      {items.length > 1 && (
        <aside className="cat-hero-rail" aria-label={t("catalog.thisWeek")}>
          <h3 className="cat-hero-rail-title">{t("catalog.thisWeek")}</h3>
          {items.map((a) => {
            const when = countdownText(countdownParts(a.next_airing_at, now));
            const src = a.cover_url ? resolveCoverUrl(a.cover_url) : null;
            return (
              <button
                key={a.id}
                type="button"
                className={`cat-hero-rail-item${a.id === hero.id ? " active" : ""}`}
                aria-pressed={a.id === hero.id}
                onClick={() => setPickedId(a.id)}
              >
                {src ? <img src={src} alt="" /> : <span className="cat-hero-rail-ph" aria-hidden="true" />}
                <span className="cat-hero-rail-text">
                  <span className="cat-hero-rail-name">{a.title}</span>
                  <span className="cat-hero-rail-when">
                    {a.next_episode != null && when
                      ? t("catalog.railEpisodeIn", { ep: a.next_episode, when })
                      : t("catalog.heroAiring")}
                  </span>
                </span>
              </button>
            );
          })}
        </aside>
      )}
    </section>
  );
}
