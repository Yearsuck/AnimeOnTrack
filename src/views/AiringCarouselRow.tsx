import { useRef, useState, useEffect } from "react";
import { useT } from "../i18n";
import { countdownLabel } from "./AiringGrid";
import type { AiringItem } from "../types";

interface AiringCarouselRowProps {
  title: string;
  items: AiringItem[];
  onOpenSeries: (s: AiringItem["series"]) => void;
}

export function AiringCarouselRow({ title, items, onOpenSeries }: AiringCarouselRowProps) {
  const t = useT();
  const scrollRef = useRef<HTMLDivElement>(null);
  const [canScrollLeft, setCanScrollLeft] = useState(false);
  const [canScrollRight, setCanScrollRight] = useState(true);

  function updateScrollButtons() {
    const el = scrollRef.current;
    if (!el) return;
    const { scrollLeft, scrollWidth, clientWidth } = el;
    setCanScrollLeft(scrollLeft > 4);
    setCanScrollRight(scrollLeft + clientWidth < scrollWidth - 4);
  }

  useEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    updateScrollButtons();
    el.addEventListener("scroll", updateScrollButtons, { passive: true });
    return () => el.removeEventListener("scroll", updateScrollButtons);
  }, []);

  function scrollByViewport(direction: -1 | 1) {
    const el = scrollRef.current;
    if (!el) return;
    el.scrollBy({ left: direction * el.clientWidth, behavior: "smooth" });
  }

  if (items.length === 0) return null;

  return (
    <section className="airing-carousel-row" aria-labelledby="carousel-title">
      <div className="airing-carousel-head">
        <h3 id="carousel-title" className="section-title">{title}</h3>
        <div className="airing-carousel-nav">
          <button
            type="button"
            className="carousel-btn carousel-btn-prev"
            aria-label={t("airing.carousel.prev")}
            onClick={() => scrollByViewport(-1)}
            disabled={!canScrollLeft}
          >
            ‹
          </button>
          <button
            type="button"
            className="carousel-btn carousel-btn-next"
            aria-label={t("airing.carousel.next")}
            onClick={() => scrollByViewport(1)}
            disabled={!canScrollRight}
          >
            ›
          </button>
        </div>
      </div>
      <div
        className="airing-carousel-track"
        ref={scrollRef}
        role="list"
        aria-label={title}
      >
        {items.map(({ series: s }) => {
          const cd = countdownLabel(s.next_episode_at);
          return (
            <div key={s.id} className="card carousel-card" role="listitem" onClick={() => onOpenSeries(s)}>
              <div className="poster">
                {s.followed && <span className="chip">{t("airing.followingChip")}</span>}
                {cd && <span className="chip chip-countdown">{cd}</span>}
                {s.cover_url ? (
                  <img src={s.cover_url} alt={s.title} loading="lazy" />
                ) : null}
              </div>
              <div className="card-body">
                <div className="card-title">{s.title}</div>
              </div>
            </div>
          );
        })}
      </div>
    </section>
  );
}
