import { useEffect, useState } from "react";
import { useT } from "../../i18n";
import { formatLabel } from "../../lib/formatLabel";
import { resolveCoverUrl } from "../../lib/posterFallback";
import { isMobile } from "../../lib/platform";
import type { CatalogAnime } from "../../types";
import { countdown } from "../../lib/airCountdown";

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
  return (airAt: number | null | undefined, now: number): string | null => {
    return countdown(airAt, now, { t });
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
  onInfo: (a: CatalogAnime, origin?: Element) => void;
}) {
  const t = useT();
  const now = useNowSecs();
  const countdownText = useCountdownText();
  const [pickedId, setPickedId] = useState<number | null>(null);
  // Banner being replaced: kept underneath until the new one has faded in.
  const [outgoing, setOutgoing] = useState<string | null>(null);
  // Cover/text swap class stays on once the rail was used (it only matters when
  // the keyed elements remount); the new banner only fades in once it loaded.
  const [swapped, setSwapped] = useState(false);
  const [readyId, setReadyId] = useState<number | null>(null);
  const hero = items.find((a) => a.id === pickedId) ?? items[0];
  if (!hero) return null;

  const banner = hero.banner_url || hero.cover_url;
  const bannerSrc = banner ? resolveCoverUrl(banner) : null;
  const coverSrc = hero.cover_url ? resolveCoverUrl(hero.cover_url) : null;
  const left = countdownText(hero.next_airing_at, now);
  const facts = [
    hero.format ? formatLabel(t, hero.format) : null,
    hero.episodes ? t("catalog.episodesCount", { count: hero.episodes }) : null,
    hero.studio,
    hero.average_score ? `${hero.average_score}%` : null,
  ]
    .filter(Boolean)
    .join(" · ");

  const railItems = items.map((a) => {
    const when = countdownText(a.next_airing_at, now);
    const src = a.cover_url ? resolveCoverUrl(a.cover_url) : null;
    return (
      <button
        key={a.id}
        type="button"
        className={`cat-hero-rail-item${a.id === hero.id ? " active" : ""}`}
        aria-pressed={a.id === hero.id}
        onClick={() => {
          if (a.id === hero.id) return;
          const next = a.banner_url || a.cover_url;
          setOutgoing(next && bannerSrc ? bannerSrc : null);
          setSwapped(true);
          setPickedId(a.id);
        }}
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
  });

  return (
    <section className="cat-hero" aria-label={t("catalog.heroAria")}>
      {bannerSrc && outgoing && outgoing !== bannerSrc && <img className="cat-hero-banner" src={outgoing} alt="" aria-hidden="true" />}
      {bannerSrc && (
        <img
          key={`banner-${hero.id}`}
          className={`cat-hero-banner${
            outgoing === null ? "" : readyId === hero.id ? " cat-hero-banner-in" : " cat-hero-banner-wait"
          }`}
          src={bannerSrc}
          alt=""
          onLoad={() => setReadyId(hero.id)}
          onError={() => setOutgoing(null)}
          onAnimationEnd={() => setOutgoing(null)}
        />
      )}
      <div className="cat-hero-scrim" aria-hidden="true" />
      <div className="cat-hero-main">
        {coverSrc && <img key={`cover-${hero.id}`} className={`cat-hero-cover${swapped ? " cat-hero-swap" : ""}`} src={coverSrc} alt="" />}
        <div key={`text-${hero.id}`} className={`cat-hero-text${swapped ? " cat-hero-swap" : ""}`}>
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
            <button type="button" className="btn btn-ghost cat-hero-btn-ghost" onClick={(e) => onInfo(hero, e.currentTarget)}>
              {t("catalog.heroInfo")}
            </button>
          </div>
        </div>
      </div>
      {items.length > 1 && (
        <aside className="cat-hero-rail" aria-label={t("catalog.thisWeek")}>
          <h3 className="cat-hero-rail-title">{t("catalog.thisWeek")}</h3>
          {isMobile ? <div className="cat-hero-rail-track">{railItems}</div> : railItems}
        </aside>
      )}
    </section>
  );
}
