// Minimal placeholder detail: shows a series title. Expand later to list its
// episode history. Kept intentionally small for v1.
import type { Series } from "../types";

export function SeriesDetail({ series }: { series: Series }) {
  return (
    <div style={{ padding: 16 }}>
      <h2>{series.title}</h2>
      <a href={series.url} target="_blank" rel="noreferrer">
        Open series page
      </a>
    </div>
  );
}
