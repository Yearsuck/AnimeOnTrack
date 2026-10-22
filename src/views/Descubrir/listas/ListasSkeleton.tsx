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
    // Same grid and card box as the loaded cards, so nothing reflows on arrival.
    <div className="grid" aria-busy="true">
      {Array.from({ length: 8 }).map((_, i) => (
        <div key={i} className="card">
          <div className="poster skeleton" />
          <div className="card-body">
            <div className="skeleton skeleton-text listas-skeleton-line-80" />
            <div className="skeleton skeleton-text listas-skeleton-line-50" />
          </div>
        </div>
      ))}
    </div>
  );
}
