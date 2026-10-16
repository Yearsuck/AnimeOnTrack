const UNITS = ["B", "KB", "MB", "GB", "TB", "PB"] as const;

/**
 * Format byte count into human-readable representation using 1024 base.
 * Uses 0 decimals for bytes, and up to 2 decimals for KB/MB/GB/TB.
 * Uses Intl.NumberFormat(locale) for locale-aware decimal separators.
 */
export function formatBytes(bytes: number, locale = "en"): string {
  if (!Number.isFinite(bytes) || bytes <= 0) {
    return "0 B";
  }

  let unitIndex = 0;
  let val = bytes;

  while (val >= 1024 && unitIndex < UNITS.length - 1) {
    val /= 1024;
    unitIndex++;
  }

  // If rounding to 2 decimal places rounds up to 1024, bump to next unit
  if (unitIndex > 0 && Math.round(val * 100) / 100 >= 1024 && unitIndex < UNITS.length - 1) {
    val /= 1024;
    unitIndex++;
  }

  const maxDecimals = unitIndex === 0 ? 0 : 2;
  const formatted = new Intl.NumberFormat(locale, {
    minimumFractionDigits: 0,
    maximumFractionDigits: maxDecimals,
  }).format(val);

  return `${formatted} ${UNITS[unitIndex]}`;
}
