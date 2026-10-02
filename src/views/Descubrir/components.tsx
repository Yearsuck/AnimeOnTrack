import { useRef, useState } from "react";
import type { Series } from "../../types";
import { useOutsideClick } from "../../lib/useOutsideClick";
import { listasInitials } from "./helpers";
import { resolveCoverUrl, useCoverFailure } from "../../lib/posterFallback";

// ---- Shared Listas card bits (poster+initials fallback, status chip, and an
// outside-click overflow menu — same visual language as the Library cards). ----

export function PosterThumb({ series }: { series: Series }) {
  const [failed, markFailed] = useCoverFailure(series.cover_url);
  const src = resolveCoverUrl(series.cover_url);
  const showFallback = !src || failed;
  return (
    <div className="listas-poster">
      {showFallback ? (
        <div className="poster-fallback" aria-hidden="true">
          {listasInitials(series.title)}
        </div>
      ) : (
        <img src={src} alt="" loading="lazy" onError={markFailed} />
      )}
    </div>
  );
}


export function OverflowMenu({
  label,
  items,
}: {
  label: string;
  items: { label: string; onClick: () => void }[];
}) {
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement>(null);
  useOutsideClick(open, ref, () => setOpen(false));
  if (items.length === 0) return null;
  return (
    <div className="card-menu-wrap" ref={ref}>
      <button
        type="button"
        className="card-menu-btn"
        aria-label={label}
        onClick={() => setOpen((v) => !v)}
      >
        ⋯
      </button>
      {open && (
        <div className="card-menu-list">
          {items.map((it, i) => (
            <button
              key={i}
              type="button"
              className="card-menu-item"
              onClick={() => {
                setOpen(false);
                it.onClick();
              }}
            >
              {it.label}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}
