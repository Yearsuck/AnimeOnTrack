import { useCallback } from "react";
import { useLang } from "../i18n";

// Locale-aware integer formatting for the counters the stats screen shows.
//
// These are cumulative totals — episodes watched across every followed series,
// minutes of watch time — so they cross 1000 for any real user and read as a
// raw "1234" without grouping separators. The app already knows the active
// language; the only thing missing was applying it to numbers as well as to
// strings. Spanish groups with a dot ("1.234"), English with a comma
// ("1,234"), which is exactly what Intl gives us for free.
const LOCALE_BY_LANG = { es: "es-ES", en: "en-GB" } as const;

export function useFormatNumber(): (value: number) => string {
  const { lang } = useLang();
  return useCallback(
    (value: number) => {
      // Guard the inputs a division can produce upstream: NaN/Infinity would
      // otherwise render literally as "NaN" in the middle of a dashboard.
      if (!Number.isFinite(value)) return "—";
      return new Intl.NumberFormat(LOCALE_BY_LANG[lang]).format(value);
    },
    [lang]
  );
}
