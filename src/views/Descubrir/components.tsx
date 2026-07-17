import { useEffect, useRef, useState } from "react";
import type { Series } from "../../types";
import { listasInitials } from "./helpers";

// ---- Shared Listas card bits (poster+initials fallback, status chip, and an
// outside-click overflow menu — same visual language as the Library cards). ----

export function PosterThumb({ series }: { series: Series }) {
  const [failed, setFailed] = useState(false);
  const showFallback = !series.cover_url || failed;
  return (
    <div className="listas-poster">
      {showFallback ? (
        <div className="poster-fallback" aria-hidden="true">
          {listasInitials(series.title)}
        </div>
      ) : (
        <img src={series.cover_url!} alt="" loading="lazy" onError={() => setFailed(true)} />
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
  useEffect(() => {
    if (!open) return;
    function onDown(e: MouseEvent) {
      if (ref.current && !ref.current.contains(e.target as Node)) setOpen(false);
    }
    document.addEventListener("mousedown", onDown);
    return () => document.removeEventListener("mousedown", onDown);
  }, [open]);
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
