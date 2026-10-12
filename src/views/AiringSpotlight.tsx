import { useEffect, useMemo, useState, useCallback } from "react";
import { useT } from "../i18n";
import { resolveCoverUrl } from "../lib/posterFallback";
import { getSeriesBanner } from "../api";
import type { AiringItem } from "../types";
import { isWithinLastThreeMonths } from "./AiringGrid";

type AiringSpotlightProps = {
  items: AiringItem[];
  onOpenSeries: (s: AiringItem["series"]) => void;
};

// Scraped cover_url carries the site's small grid-thumbnail JetPack/Photon
// CDN size (e.g. `?resize=247,350`) — fine for a poster card, blurry blown
// up across the full-width spotlight banner. Dropping the `resize` param
// makes the same CDN serve its original, larger image.
function highResCover(url: string): string {
  try {
    const u = new URL(url);
    u.searchParams.delete("resize");
    return u.toString();
  } catch {
    return url;
  }
}

export function AiringSpotlight({ items, onOpenSeries }: AiringSpotlightProps) {
  const t = useT();
  const [currentId, setCurrentId] = useState<number | null>(null);
  const [isHovering, setIsHovering] = useState(false);
  // On-demand fetched wide banner images from AniList
  const [banners, setBanners] = useState<Record<number, string>>({});
  // If a banner image fails to load, fall back to cover poster
  const [bannerFailedIds, setBannerFailedIds] = useState<Set<number>>(new Set());
  // A series' cover_url can be a remote URL the app's CSP blocks —
  // tracked so a blocked slide degrades gracefully to the ambient gradient
  const [failedIds, setFailedIds] = useState<Set<string>>(new Set());

  // Prioritize anime of the current season that the user DOES NOT follow,
  // ordered by highest AniList score, then next upcoming episode.
  const featuredItems = useMemo(() => {
    // Only series the user DOES NOT follow:
    const notFollowed = items.filter((it) => !it.series.followed);
    const candidatePool = notFollowed.length > 0 ? notFollowed : items;

    const sorted = [...candidatePool].sort((a, b) => {
      const aIsSeason = a.first_episode_at != null && isWithinLastThreeMonths(a.first_episode_at);
      const bIsSeason = b.first_episode_at != null && isWithinLastThreeMonths(b.first_episode_at);
      if (aIsSeason !== bIsSeason) {
        return aIsSeason ? -1 : 1;
      }
      const scoreA = a.score ?? -1;
      const scoreB = b.score ?? -1;
      if (scoreB !== scoreA) {
        return scoreB - scoreA;
      }
      const aTime = a.series.next_episode_at ?? Infinity;
      const bTime = b.series.next_episode_at ?? Infinity;
      return aTime - bTime;
    });

    return sorted.slice(0, 4);
  }, [items]);

  // Fetch missing banners on demand for featured carousel items
  useEffect(() => {
    featuredItems.forEach((item) => {
      const hasBanner = item.banner_url || banners[item.series.id];
      if (!hasBanner && !bannerFailedIds.has(item.series.id)) {
        getSeriesBanner(item.series.id)
          .then((b) => {
            if (b) {
              setBanners((prev) => ({ ...prev, [item.series.id]: b }));
            }
          })
          .catch(() => {});
      }
    });
  }, [featuredItems, banners, bannerFailedIds]);

  const currentIndex = useMemo(() => {
    if (featuredItems.length === 0) return 0;
    const idx = featuredItems.findIndex((it) => it.series.id === currentId);
    return idx >= 0 ? idx : 0;
  }, [featuredItems, currentId]);

  const goTo = useCallback((idx: number) => {
    if (featuredItems.length === 0) return;
    const next = ((idx % featuredItems.length) + featuredItems.length) % featuredItems.length;
    setCurrentId(featuredItems[next].series.id);
  }, [featuredItems]);

  const goPrev = useCallback(() => goTo(currentIndex - 1), [currentIndex, goTo]);
  const goNext = useCallback(() => goTo(currentIndex + 1), [currentIndex, goTo]);

  useEffect(() => {
    if (isHovering || featuredItems.length === 0) return;
    const id = setInterval(() => goNext(), 6000);
    return () => clearInterval(id);
  }, [isHovering, goNext, featuredItems.length]);

  if (featuredItems.length === 0) return null;

  return (
    <div
      className="spotlight"
      role="region"
      aria-label={t("spotlight.ariaLabel")}
      onMouseEnter={() => setIsHovering(true)}
      onMouseLeave={() => setIsHovering(false)}
    >
      <div
        className="spotlight-track"
        style={{ transform: `translateX(-${currentIndex * (100 / featuredItems.length)}%)` }}
      >
        {featuredItems.map((item) => {
          // Priority 1: Wide horizontal banner from AniList
          const banner = !bannerFailedIds.has(item.series.id)
            ? (banners[item.series.id] || item.banner_url)
            : null;

          const isBanner = Boolean(banner);
          const rawCover = item.series.cover_url ?? "";
          const resolvedCover = item.series.cover_url
            ? resolveCoverUrl(/^https?:/i.test(rawCover) ? highResCover(rawCover) : rawCover)
            : null;

          const bannerSrc = isBanner ? banner : null;
          const bgSrc = bannerSrc || resolvedCover;
          const coverKey = `${item.series.id}|${rawCover}`;
          const isFailed = failedIds.has(coverKey);

          const isSeason =
            item.first_episode_at != null && isWithinLastThreeMonths(item.first_episode_at);

          return (
            <div key={item.series.id} className="spotlight-slide">
              {/* Wide landscape banner if present */}
              {bannerSrc && (
                <img
                  className="spotlight-bg"
                  src={bannerSrc}
                  alt=""
                  aria-hidden="true"
                  onError={() => {
                    setBannerFailedIds((prev) => new Set(prev).add(item.series.id));
                  }}
                />
              )}

              {/* Ambient blurred backdrop when no wide banner exists */}
              {!bannerSrc && bgSrc && !isFailed && (
                <img
                  className="spotlight-blur-bg"
                  src={bgSrc}
                  alt=""
                  aria-hidden="true"
                  onError={() => {
                    setFailedIds((prev) => (prev.has(coverKey) ? prev : new Set(prev).add(coverKey)));
                  }}
                />
              )}

              <div className="spotlight-scrim" />

              <div className="spotlight-content">
                {/* Crisp vertical poster thumbnail card when wide banner is absent */}
                {!bannerSrc && resolvedCover && !isFailed && (
                  <div
                    className="spotlight-poster-card clickable"
                    onClick={() => onOpenSeries(item.series)}
                    title={item.series.title}
                  >
                    <img src={resolvedCover} alt="" />
                  </div>
                )}

                <div className="spotlight-info">
                  <div className="spotlight-badges">
                    {isSeason && (
                      <span className="spotlight-badge badge-season">
                        {t("spotlight.thisSeason") || "Esta temporada"}
                      </span>
                    )}
                    {item.score != null && item.score > 0 && (
                      <span className="spotlight-badge badge-score">
                        ★ {Math.round(item.score)}%
                      </span>
                    )}
                  </div>
                  <h3 className="spotlight-title">{item.series.title}</h3>
                  <button
                    className="btn btn-primary spotlight-btn"
                    onClick={(e) => {
                      e.stopPropagation();
                      onOpenSeries(item.series);
                    }}
                  >
                    {t("spotlight.watchBtn")}
                  </button>
                </div>
              </div>
            </div>
          );
        })}
      </div>

      <div className="spotlight-controls">
        <button
          className="spotlight-nav spotlight-prev"
          onClick={goPrev}
          aria-label={t("spotlight.prevBtn")}
        >
          ‹
        </button>
        <div className="spotlight-dots" role="tablist" aria-label={t("spotlight.dotsAria")}>
          {featuredItems.map((_, idx) => (
            <button
              key={idx}
              role="tab"
              aria-selected={idx === currentIndex}
              aria-label={`${t("spotlight.slideAria")} ${idx + 1}`}
              className={`spotlight-dot${idx === currentIndex ? " active" : ""}`}
              onClick={() => goTo(idx)}
            />
          ))}
        </div>
        <button
          className="spotlight-nav spotlight-next"
          onClick={goNext}
          aria-label={t("spotlight.nextBtn")}
        >
          ›
        </button>
      </div>
    </div>
  );
}