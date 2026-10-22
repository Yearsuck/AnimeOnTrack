// The detail view's AniList link must only ever open anilist.co. It used to
// fall back to the series' own (pirate-site) URL under the "Ver en AniList"
// label whenever the catalog info was missing or still loading.

export function isAniListUrl(url: string | null | undefined): boolean {
  if (!url) return false;
  try {
    const u = new URL(url);
    return u.protocol === "https:" && (u.hostname === "anilist.co" || u.hostname.endsWith(".anilist.co"));
  } catch {
    return false;
  }
}

export function aniListSearchUrl(title: string): string {
  return `https://anilist.co/search/anime?search=${encodeURIComponent(title.trim())}`;
}

export type AniListTarget =
  | { kind: "loading" }
  | { kind: "open"; url: string }
  | { kind: "search"; url: string };

/** What the AniList link does: nothing yet while the catalog lookup runs,
 *  open the matched entry, or search AniList by title when unmatched. */
export function aniListTarget(
  loaded: boolean,
  catalogUrl: string | null | undefined,
  title: string
): AniListTarget {
  if (!loaded) return { kind: "loading" };
  if (isAniListUrl(catalogUrl)) return { kind: "open", url: catalogUrl as string };
  return { kind: "search", url: aniListSearchUrl(title) };
}

/** The series' own page is worth a separate link only when it is a real
 *  site page (catalog-only rows point at AniList already). */
export function sitePageUrl(seriesUrl: string | null | undefined): string | null {
  if (!seriesUrl || isAniListUrl(seriesUrl)) return null;
  try {
    const u = new URL(seriesUrl);
    return u.protocol === "https:" || u.protocol === "http:" ? seriesUrl : null;
  } catch {
    return null;
  }
}

/** AniList season of a premiere timestamp (unix seconds, UTC). */
export function seasonOf(ts: number | null | undefined): { season: "winter" | "spring" | "summer" | "fall"; year: number } | null {
  if (ts == null) return null;
  const d = new Date(ts * 1000);
  if (Number.isNaN(d.getTime())) return null;
  const m = d.getUTCMonth();
  const season = m < 3 ? "winter" : m < 6 ? "spring" : m < 9 ? "summer" : "fall";
  return { season, year: d.getUTCFullYear() };
}
