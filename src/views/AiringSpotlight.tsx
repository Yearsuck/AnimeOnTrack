import { useEffect, useMemo, useState, useCallback } from "react";
import { useT } from "../i18n";
import type { AiringItem } from "../types";

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
  const [currentIndex, setCurrentIndex] = useState(0);
  const [isHovering, setIsHovering] = useState(false);

  const featuredItems = useMemo(() => {
    const withCover = items.filter((it) => it.series.cover_url);
    if (withCover.length === 0) return items.slice(0, 4);
    const sorted = [...withCover].sort((a, b) => {
      const aTime = a.series.next_episode_at ?? Infinity;
      const bTime = b.series.next_episode_at ?? Infinity;
      return aTime - bTime;
    });
    return sorted.slice(0, 4);
  }, [items]);

  const goTo = useCallback((idx: number) => {
    setCurrentIndex((idx + featuredItems.length) % featuredItems.length);
  }, [featuredItems.length]);

  const goPrev = useCallback(() => goTo(currentIndex - 1), [currentIndex, goTo]);
  const goNext = useCallback(() => goTo(currentIndex + 1), [currentIndex, goTo]);

  useEffect(() => {
    if (isHovering || featuredItems.length === 0) return;
    const id = setInterval(() => goNext(), 6000);
    return () => clearInterval(id);
  }, [isHovering, goNext, featuredItems.length]);

  // All hooks above must run unconditionally on every render (Rules of
  // Hooks) — `items` arrives async from AiringGrid, so featuredItems can be
  // empty on the first render and non-empty later; an early return before
  // the hooks would change the hook count between renders and crash React.
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
        {featuredItems.map((item) => (
          <div key={item.series.id} className="spotlight-slide">
            {item.series.cover_url && (
              <img
                className="spotlight-bg"
                src={highResCover(item.series.cover_url)}
                alt=""
                aria-hidden="true"
              />
            )}
            <div className="spotlight-scrim" />
            <div className="spotlight-content">
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
        ))}
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