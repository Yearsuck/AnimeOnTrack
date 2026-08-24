// `episode.released_at` is scraped verbatim from the site's own HTML — not
// something the app generates — so it always arrives as the site's own
// Spanish free text ("Hace 2 dias", "junio 29, 2026") or, for JKanime, an
// ISO-ish datetime string. Rendering that raw text directly (as Pending.tsx
// and SeriesDetail.tsx used to) shows Spanish regardless of the app's
// language setting, since the source site has no concept of it.
//
// This parses those known shapes into a Unix timestamp (seconds) so the
// caller can hand it to `countdownLabel` for a properly localized "X ago"
// string from the app's own i18n catalog — never native Intl locale data,
// which WebView2's bundled ICU doesn't reliably cover for every language
// the app supports (see AiringGrid.tsx's getWeekdayName fix). Returns null
// for anything unrecognized so the caller can fall back to the raw text
// rather than showing nothing.

const SPANISH_MONTHS: Record<string, number> = {
  enero: 0,
  febrero: 1,
  marzo: 2,
  abril: 3,
  mayo: 4,
  junio: 5,
  julio: 6,
  agosto: 7,
  septiembre: 8,
  setiembre: 8,
  octubre: 9,
  noviembre: 10,
  diciembre: 11,
};

const RELATIVE_UNIT_MS: Record<string, number> = {
  segundo: 1_000,
  minuto: 60_000,
  hora: 3_600_000,
  dia: 86_400_000,
  "día": 86_400_000,
  semana: 604_800_000,
  mes: 2_592_000_000, // ~30 days, the site's own text is this coarse too
  "año": 31_536_000_000,
};

export function parseReleasedAtToUnixSeconds(raw: string): number | null {
  const s = raw.trim().toLowerCase();

  // "hace 52 minutos" / "hace 2 dias" / "hace 1 hora"
  const rel = s.match(/^hace\s+(\d+)\s+(segundo|minuto|hora|d[ií]a|semana|mes|a[ñn]o)s?$/);
  if (rel) {
    const n = parseInt(rel[1], 10);
    const unitMs = RELATIVE_UNIT_MS[rel[2].replace("n", "ñ")] ?? RELATIVE_UNIT_MS[rel[2]];
    if (unitMs == null) return null;
    return Math.round((Date.now() - n * unitMs) / 1000);
  }

  // "junio 29, 2026" (Spanish month name, case-insensitive)
  const abs = s.match(/^([a-záéíóúñ]+)\s+(\d{1,2}),\s*(\d{4})$/);
  if (abs) {
    const month = SPANISH_MONTHS[abs[1]];
    if (month == null) return null;
    const d = new Date(Number(abs[3]), month, Number(abs[2]));
    return Math.round(d.getTime() / 1000);
  }

  // JKanime: "2026-07-11 17:47:15"
  const iso = s.match(/^(\d{4})-(\d{2})-(\d{2})[ t](\d{2}):(\d{2}):(\d{2})$/);
  if (iso) {
    const d = new Date(
      Number(iso[1]),
      Number(iso[2]) - 1,
      Number(iso[3]),
      Number(iso[4]),
      Number(iso[5]),
      Number(iso[6])
    );
    return Math.round(d.getTime() / 1000);
  }

  return null;
}
