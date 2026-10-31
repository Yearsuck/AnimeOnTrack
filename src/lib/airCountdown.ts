import type { MessageKey } from "../i18n";
export type CountdownOptions = {
  /** If true, prefixes "≈ " to the returned countdown label */
  estimated?: boolean;
  /** Translation function, e.g. from useT() */
  t?: (key: MessageKey, params?: Record<string, string | number>) => string;
};

/**
 * Shared countdown formatter.
 * - Under 1 hour -> minutes (floor everywhere, e.g. "30 min")
 * - Under 24 hours (1 day) -> hours + minutes (floor, e.g. "1 h 45 min")
 * - 1 day or more -> days + hours (floor, e.g. "2 d 3 h")
 * - Elapsed (airAt <= now) -> null (never "hace 2 h")
 * - If estimated is true -> prefixes "≈ "
 */
export function countdown(
  airAt: number | null | undefined,
  now: number,
  options?: CountdownOptions
): string | null {
  if (airAt == null || airAt <= now) {
    return null;
  }

  const diff = airAt - now;

  let text: string;
  if (diff < 3600) {
    const m = Math.floor(diff / 60);
    text = options?.t ? options.t("catalog.countdownMinutes", { m }) : `${m} min`;
  } else if (diff < 86400) {
    const h = Math.floor(diff / 3600);
    const m = Math.floor((diff % 3600) / 60);
    text = options?.t ? options.t("catalog.countdownHours", { h, m }) : `${h} h ${m} min`;
  } else {
    const d = Math.floor(diff / 86400);
    const h = Math.floor((diff % 86400) / 3600);
    text = options?.t ? options.t("catalog.countdownDays", { d, h }) : `${d} d ${h} h`;
  }

  return options?.estimated ? `≈ ${text}` : text;
}

/**
 * Formats watch progress as "seen / aired" where aired = episodes known to exist (scraped rows),
 * plus the planned total in parentheses when the catalog knows a larger total: "4 / 4 (12)".
 * E.g.:
 * - (4, 4, 12) -> "4 / 4 (12)"
 * - (4, 4, null) -> "4 / 4"
 * - (4, 4, 4) -> "4 / 4"
 */
/** Episodes known to exist, plus the planned total in parentheses when the catalog knows a larger one: "4 (12)". */
export function formatAired(aired: number, planned?: number | null): string {
  return planned != null && planned > aired ? `${aired} (${planned})` : `${aired}`;
}

export function formatProgress(seen: number, aired: number, planned?: number | null): string {
  return `${seen} / ${formatAired(aired, planned)}`;
}
