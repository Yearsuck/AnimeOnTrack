export type AirCountdownResult = {
  unit: "minutes" | "hours" | "days";
  value: number;
};

/**
 * Calculates countdown units and value until `airAt` (both timestamps in seconds).
 * Returns null when `airAt <= now`.
 * - Under 1 hour -> minutes (at least 1, rounded up)
 * - Under 48 hours -> hours (rounded down, at least 1)
 * - Otherwise -> days (rounded down)
 */
export function airCountdown(airAt: number, now: number): AirCountdownResult | null {
  if (airAt <= now) {
    return null;
  }

  const diff = airAt - now;

  if (diff < 3600) {
    return {
      unit: "minutes",
      value: Math.max(1, Math.ceil(diff / 60)),
    };
  }

  if (diff < 48 * 3600) {
    return {
      unit: "hours",
      value: Math.max(1, Math.floor(diff / 3600)),
    };
  }

  return {
    unit: "days",
    value: Math.floor(diff / 86400),
  };
}
