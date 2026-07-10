import type { Series } from "../types";

// decide_catalog_card always writes the synthetic slug `anilist-{id}`;
// link_catalog_series rewrites it to the site's real slug on a successful
// match. A real site slug starting with this literal is not a realistic
// collision, so this doubles as a cheap "still unlinked?" signal the
// frontend can read straight off `Series` without a dedicated field. Shared
// between Descubrir.tsx (Listas retry button) and SeriesDetail.tsx (link
// on open of an unlinked catalog row).
export function isUnlinkedCatalogRow(series: Series): boolean {
  return series.slug.startsWith("anilist-");
}
