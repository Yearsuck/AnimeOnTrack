import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";
import { getAnimeCatalog } from "../../api";
import { useT } from "../../i18n";
import { PosterCard, posterMetaLine } from "../../components/PosterCard";
import type { CatalogAnime, CatalogFilter } from "../../types";

// Fetched rows per filter, kept across browse/results trips so returning to
// browse does not replay every shelf's skeleton. Cleared after a sync.
const shelfCache = new Map<string, CatalogAnime[]>();
export function clearShelfCache() {
  shelfCache.clear();
}

/**
 * One horizontal row of Explorar's browse mode. Fetches page 1 of `filter`
 * only once the row comes near the viewport (rows further down cost nothing
 * until scrolled to), unless the caller already holds the rows (`items`).
 * Clicking a card opens its info; "Ver todo" hands the same filter to the
 * results grid.
 */
export function CatalogShelf({
  title,
  hint,
  filter,
  items: given,
  badgeFor,
  onOpen,
  onSeeAll,
}: {
  title: string;
  hint?: string;
  filter: CatalogFilter;
  /** Rows already fetched by the caller; skips the shelf's own fetch. */
  items?: CatalogAnime[] | null;
  badgeFor?: (a: CatalogAnime) => ReactNode;
  onOpen: (a: CatalogAnime) => void;
  onSeeAll: () => void;
}) {
  const t = useT();
  const sectionRef = useRef<HTMLElement>(null);
  const trackRef = useRef<HTMLDivElement>(null);
  const filterKey = JSON.stringify(filter);
  const [near, setNear] = useState(false);
  const [fetched, setFetched] = useState<CatalogAnime[] | null>(() => shelfCache.get(filterKey) ?? null);
  const [failed, setFailed] = useState(false);
  const [edges, setEdges] = useState({ start: true, end: false });

  useEffect(() => {
    const el = sectionRef.current;
    if (!el || near) return;
    const io = new IntersectionObserver(
      (entries) => {
        if (entries.some((e) => e.isIntersecting)) setNear(true);
      },
      { rootMargin: "400px 0px" }
    );
    io.observe(el);
    return () => io.disconnect();
  }, [near]);

  // Keyed on the filter's content so a re-render with an equal object does
  // not refetch.
  useEffect(() => {
    if (given !== undefined || !near || fetched !== null) return;
    let alive = true;
    setFailed(false);
    getAnimeCatalog(1, filter)
      .then((page) => {
        shelfCache.set(filterKey, page.items);
        if (alive) setFetched(page.items);
      })
      .catch((e) => {
        console.error("catalog shelf load failed", e);
        if (alive) setFailed(true);
      });
    return () => {
      alive = false;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [filterKey, near, given]);

  const items = given !== undefined ? given : fetched;

  const updateEdges = useCallback(() => {
    const el = trackRef.current;
    if (!el) return;
    setEdges({ start: el.scrollLeft <= 1, end: el.scrollLeft + el.clientWidth >= el.scrollWidth - 1 });
  }, []);
  useEffect(updateEdges, [items, updateEdges]);

  function scrollPage(dir: 1 | -1) {
    const el = trackRef.current;
    if (!el) return;
    el.scrollBy({ left: dir * el.clientWidth * 0.85, behavior: "smooth" });
  }

  if (items && items.length === 0) return null;

  return (
    <section ref={sectionRef} className="cat-shelf" aria-label={title}>
      <div className="cat-shelf-head">
        <h2 className="cat-shelf-title">{title}</h2>
        {hint && <span className="cat-shelf-hint">{hint}</span>}
        <div className="spacer" />
        <button type="button" className="btn btn-ghost btn-sm" onClick={onSeeAll}>
          {t("catalog.seeAll")}
        </button>
        <div className="cat-shelf-arrows">
          <button
            type="button"
            className="cat-shelf-arrow"
            onClick={() => scrollPage(-1)}
            disabled={edges.start}
            aria-label={t("catalog.scrollPrev")}
          >
            ‹
          </button>
          <button
            type="button"
            className="cat-shelf-arrow"
            onClick={() => scrollPage(1)}
            disabled={edges.end}
            aria-label={t("catalog.scrollNext")}
          >
            ›
          </button>
        </div>
      </div>
      {failed ? (
        <div className="muted cat-shelf-note">{t("catalog.shelfError")}</div>
      ) : !items ? (
        <div className="cat-shelf-track" aria-busy="true">
          {Array.from({ length: 7 }, (_, i) => (
            <div key={i} className="cat-shelf-skeleton" />
          ))}
        </div>
      ) : (
        <div ref={trackRef} className="cat-shelf-track" onScroll={updateEdges}>
          {items.map((a) => (
            <PosterCard
              key={a.id}
              title={a.title}
              coverUrl={a.cover_url}
              format={a.format}
              genres={a.genres}
              meta={posterMetaLine(t, { episodes: a.episodes, score: a.average_score })}
              badge={badgeFor?.(a)}
              onActivate={() => onOpen(a)}
            />
          ))}
        </div>
      )}
    </section>
  );
}
