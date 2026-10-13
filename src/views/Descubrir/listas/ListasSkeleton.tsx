import type { ViewMode } from "./types";

export function ListasSkeleton({ viewMode }: { viewMode: ViewMode }) {
  if (viewMode === "list") {
    return (
      <div className="listas-dense-container" aria-busy="true">
        {Array.from({ length: 6 }).map((_, i) => (
          <div key={i} className="listas-dense-row-item">
            <div className="listas-dense-thumb listas-skeleton-dense-thumb" />
            <div className="listas-skeleton-dense-main">
              <div className="listas-skeleton-dense-title" />
              <div className="listas-skeleton-dense-sub" />
            </div>
          </div>
        ))}
      </div>
    );
  }

  return (
    <div className="listas-skeleton-grid" aria-busy="true">
      {Array.from({ length: 8 }).map((_, i) => (
        <div key={i} className="listas-skeleton-card">
          <div className="listas-skeleton-poster" />
          <div className="listas-skeleton-lines">
            <div className="listas-skeleton-line listas-skeleton-line-80" />
            <div className="listas-skeleton-line listas-skeleton-line-50" />
          </div>
        </div>
      ))}
    </div>
  );
}
