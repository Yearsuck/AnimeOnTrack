import type { KeyboardEvent, ReactNode, SyntheticEvent } from "react";
import { useT, type MessageKey } from "../i18n";
import { formatLabel } from "../lib/formatLabel";
import { initials, resolveCoverUrl, useCoverFailure } from "../lib/posterFallback";

/**
 * The meta line every Catálogo poster card shows: "25 episodios · 85%", or
 * "En emisión · 85%" when the episode count is still unknown on an airing
 * show. Unknown counts on finished shows are simply left out.
 */
export function posterMetaLine(
  t: (key: MessageKey, params?: Record<string, string | number>) => string,
  { episodes, airing, score }: { episodes: number | null | undefined; airing?: boolean; score: number | null | undefined }
): string {
  const parts: string[] = [];
  if (episodes != null && episodes > 0) {
    parts.push(episodes === 1 ? t("catalog.episodesOne") : t("catalog.episodesCount", { count: episodes }));
  }
  if (airing) parts.push(t("airing.airingNow"));
  if (score != null && score > 0) parts.push(`${score}%`);
  return parts.join(" · ");
}

/** Stops a nested control's click/keys from reaching the card's own handler. */
export function stopCardEvent(e: SyntheticEvent) {
  e.stopPropagation();
}

/**
 * One poster card for Catálogo (Explorar and Listas): format chip top-left,
 * a single control (info / overflow menu) top-right, an optional primary
 * action revealed on hover/focus at the poster's foot, and title + genres +
 * meta below. The whole card is keyboard-operable (Enter/Space).
 */
export function PosterCard({
  title,
  coverUrl,
  format,
  genres,
  meta,
  selected = false,
  selectable = false,
  onActivate,
  corner,
  action,
  badge,
  footer,
}: {
  title: string;
  coverUrl: string | null | undefined;
  format?: string | null;
  genres: string[];
  meta: string;
  selected?: boolean;
  /** Clicking toggles selection (adds the pressed state for assistive tech). */
  selectable?: boolean;
  onActivate: () => void;
  corner?: ReactNode;
  action?: ReactNode;
  /** A short label pinned to the poster's bottom-left (countdown, score). */
  badge?: ReactNode;
  footer?: ReactNode;
}) {
  const t = useT();
  const src = resolveCoverUrl(coverUrl ?? null);
  const [failed, markFailed] = useCoverFailure(coverUrl ?? null);
  const showFallback = !src || failed;

  function onKeyDown(e: KeyboardEvent<HTMLDivElement>) {
    if (e.target !== e.currentTarget) return;
    if (e.key === "Enter" || e.key === " ") {
      e.preventDefault();
      onActivate();
    }
  }

  return (
    <div
      className={`card poster-card${selectable ? " card-selectable" : ""}${selected ? " selected" : ""}`}
      role="button"
      tabIndex={0}
      aria-pressed={selectable ? selected : undefined}
      onClick={onActivate}
      onKeyDown={onKeyDown}
      title={title}
    >
      <div className="poster">
        {format && <span className="chip">{formatLabel(t, format)}</span>}
        {corner}
        {showFallback ? (
          <div className="poster-fallback" aria-hidden="true">
            {initials(title)}
          </div>
        ) : (
          <img src={src!} alt="" loading="lazy" onError={markFailed} />
        )}
        {badge && <span className="poster-badge">{badge}</span>}
        {selected && (
          <span className="card-select-check" aria-hidden="true">
            ✓
          </span>
        )}
        {action && !selectable && (
          <div className="poster-action" onClick={stopCardEvent} onKeyDown={stopCardEvent}>
            {action}
          </div>
        )}
      </div>
      <div className="card-body">
        <div className="card-title">{title}</div>
        <div className="muted card-sub">{genres.slice(0, 3).join(", ") || "—"}</div>
        {meta && <div className="muted card-sub">{meta}</div>}
        {footer}
      </div>
    </div>
  );
}
