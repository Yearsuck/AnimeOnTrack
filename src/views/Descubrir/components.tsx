import type { Series } from "../../types";
import { Menu } from "../../components/Menu";
import { listasInitials } from "./helpers";
import { resolveCoverUrl, useCoverFailure } from "../../lib/posterFallback";

// ---- Shared Listas card bits (poster+initials fallback, status chip, and an
// outside-click overflow menu — same visual language as the Library cards). ----

export function PosterThumb({ series, onClick }: { series: Series; onClick?: () => void }) {
  const [failed, markFailed] = useCoverFailure(series.cover_url);
  const src = resolveCoverUrl(series.cover_url);
  const showFallback = !src || failed;
  return (
    <div
      className={`listas-poster${onClick ? " clickable" : ""}`}
      onClick={onClick}
      role={onClick ? "button" : undefined}
      tabIndex={onClick ? 0 : undefined}
      onKeyDown={
        onClick
          ? (e) => {
              if (e.key === "Enter" || e.key === " ") {
                e.preventDefault();
                onClick();
              }
            }
          : undefined
      }
    >
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
  if (items.length === 0) return null;
  return (
    <div className="card-menu-wrap">
      <Menu
        label={label}
        items={items.map((it) => ({ label: it.label, onSelect: it.onClick }))}
      />
    </div>
  );
}
