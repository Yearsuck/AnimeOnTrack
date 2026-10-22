import type { MessageKey } from "../i18n";

// AniList `format` codes (plus the scraped sites' Spanish "Pelicula") mapped
// to one translated label, so Catálogo never shows a raw enum like TV_SHORT.
const FORMAT_KEYS: Record<string, MessageKey> = {
  TV: "format.TV",
  TV_SHORT: "format.TV_SHORT",
  MOVIE: "format.MOVIE",
  PELICULA: "format.MOVIE",
  "PELÍCULA": "format.MOVIE",
  OVA: "format.OVA",
  ONA: "format.ONA",
  SPECIAL: "format.SPECIAL",
  MUSIC: "format.MUSIC",
};

/** The catalog key for a raw format code, or null when it is not a known one. */
export function formatLabelKey(raw: string | null | undefined): MessageKey | null {
  if (!raw) return null;
  return FORMAT_KEYS[raw.trim().toUpperCase()] ?? null;
}

/** Translated format label; an unknown code is shown as it came. */
export function formatLabel(
  t: (key: MessageKey) => string,
  raw: string | null | undefined
): string {
  if (!raw) return "";
  const key = formatLabelKey(raw);
  return key ? t(key) : raw;
}

/** Display order for format pickers: AniList's own order, unknown codes last. */
export const FORMAT_ORDER = ["TV", "TV_SHORT", "MOVIE", "SPECIAL", "OVA", "ONA", "MUSIC"];

export function sortFormats(formats: Iterable<string>): string[] {
  const rank = (f: string) => {
    const i = FORMAT_ORDER.indexOf(f.toUpperCase());
    return i === -1 ? FORMAT_ORDER.length : i;
  };
  return [...new Set(formats)].sort((a, b) => rank(a) - rank(b) || a.localeCompare(b));
}
