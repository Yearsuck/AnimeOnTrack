import { DECK_PANEL_STORAGE_KEY } from "./constants";

// AniList's siteUrl shape is https://anilist.co/anime/{id}/{slug} — catalog
// cards carry that as their url, this pulls the id back out for
// decide_catalog_card (which needs it to key the synthetic series row).
export function anilistIdFromUrl(url: string): number | null {
  const m = url.match(/anilist\.co\/anime\/(\d+)/);
  return m ? Number(m[1]) : null;
}

export function listasInitials(title: string): string {
  const chars = title
    .trim()
    .split(/\s+/)
    .filter(Boolean)
    .slice(0, 2)
    .map((w) => w[0]?.toUpperCase() ?? "")
    .join("");
  return chars || "?";
}

// Accent/case-insensitive normalizer for the Listas title search. The
// replace() class is the combining-diacritics Unicode range U+0300-U+036F,
// written as escapes so it survives regardless of file encoding.
export const norm = (s: string) => s.toLowerCase().normalize("NFD").replace(/[̀-ͯ]/g, "");

export function getInitialDeckPanelOpen(): boolean {
  try {
    return localStorage.getItem(DECK_PANEL_STORAGE_KEY) !== "closed";
  } catch {
    return true;
  }
}

export function persistDeckPanelOpen(open: boolean): void {
  try {
    localStorage.setItem(DECK_PANEL_STORAGE_KEY, open ? "open" : "closed");
  } catch {
    // localStorage unavailable — the choice just won't persist.
  }
}
