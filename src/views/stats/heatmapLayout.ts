import type { DayCount } from "../../types";

export interface HeatmapDay {
  date: string;
  count: number;
  level: 0 | 1 | 2 | 3 | 4;
  dayOfWeek: number; // 0 = Mon, 1 = Tue, ..., 6 = Sun
}

export interface HeatmapWeek {
  weekIndex: number;
  days: (HeatmapDay | null)[]; // Exactly 7 entries (index 0 = Monday, ..., index 6 = Sunday)
}

export interface HeatmapMonthBoundary {
  month: number; // 0 = Jan, ..., 11 = Dec
  weekIndex: number; // Column index where this month first appears
}

export interface HeatmapYearStats {
  totalEpisodes: number;
  bestDay: { date: string; count: number } | null;
}

/**
 * Level cut-offs (inclusive upper bounds of levels 1..3) from the quartiles of
 * the year's non-zero days, so one bulk-marked day (hundreds of episodes at
 * once) does not flatten every other day into the lowest level.
 */
export function intensityThresholds(counts: number[]): [number, number, number] {
  const sorted = counts.filter((c) => c > 0).sort((a, b) => a - b);
  if (sorted.length === 0) return [1, 2, 3];
  const at = (q: number) => sorted[Math.min(sorted.length - 1, Math.floor(q * sorted.length))];
  // The busiest day must always reach level 4: cap every cut-off below the max
  // (with few active days the quartiles can equal the max, leaving it at level 1).
  const cap = sorted[sorted.length - 1] - 1;
  return [Math.min(at(0.25), cap), Math.min(at(0.5), cap), Math.min(at(0.75), cap)];
}

export function getIntensityLevel(
  count: number,
  [t1, t2, t3]: [number, number, number] = [1, 3, 5],
): 0 | 1 | 2 | 3 | 4 {
  if (count <= 0) return 0;
  if (count <= t1) return 1;
  if (count <= t2) return 2;
  if (count <= t3) return 3;
  return 4;
}

/**
 * Layout helper for GitHub-style full-year activity heatmap.
 *
 * Weekday layout math:
 * - A standard week in the heatmap has 7 rows:
 *     Row 0: Monday
 *     Row 1: Tuesday
 *     Row 2: Wednesday
 *     Row 3: Thursday
 *     Row 4: Friday
 *     Row 5: Saturday
 *     Row 6: Sunday
 *
 * - To map JavaScript's `Date.prototype.getDay()` (0 = Sunday, 1 = Monday, ..., 6 = Saturday)
 *   to our Monday-based 0..6 index:
 *     `weekday = (date.getDay() + 6) % 7`
 *   Examples:
 *     Sunday (0):    (0 + 6) % 7 = 6 (Row 6)
 *     Monday (1):    (1 + 6) % 7 = 0 (Row 0)
 *     Tuesday (2):   (2 + 6) % 7 = 1 (Row 1)
 *     Wednesday (3): (3 + 6) % 7 = 2 (Row 2)
 *
 * - For a year where January 1st falls on a Wednesday:
 *     `startOffset = (new Date(year, 0, 1).getDay() + 6) % 7` evaluates to 2.
 *   This means the first week column (week 0) has:
 *     - Row 0 (Monday): empty leading cell (null)
 *     - Row 1 (Tuesday): empty leading cell (null)
 *     - Row 2 (Wednesday): January 1st (first day of the year)
 *   Thus, the first column has exactly 2 empty leading cells (Mon, Tue).
 */
export function buildHeatmapWeeks(year: number, days: DayCount[]): HeatmapWeek[] {
  const countMap = new Map<string, number>();
  for (const d of days) {
    countMap.set(d.day, d.count);
  }

  const thresholds = intensityThresholds(days.map((d) => d.count));

  // Jan 1st day of week in Monday=0..Sunday=6 system
  const jan1 = new Date(year, 0, 1);
  const startOffset = (jan1.getDay() + 6) % 7;

  // Leap year calculation
  const isLeap = (year % 4 === 0 && year % 100 !== 0) || year % 400 === 0;
  const daysInYear = isLeap ? 366 : 365;

  const totalWeeks = Math.ceil((startOffset + daysInYear) / 7);
  const weeks: HeatmapWeek[] = [];

  for (let w = 0; w < totalWeeks; w++) {
    weeks.push({
      weekIndex: w,
      days: [null, null, null, null, null, null, null],
    });
  }

  for (let i = 0; i < daysInYear; i++) {
    const d = new Date(year, 0, 1 + i);
    const m = String(d.getMonth() + 1).padStart(2, "0");
    const dayNum = String(d.getDate()).padStart(2, "0");
    const iso = `${year}-${m}-${dayNum}`;
    const count = countMap.get(iso) ?? 0;

    const cellIndex = startOffset + i;
    const weekIndex = Math.floor(cellIndex / 7);
    const dayOfWeek = cellIndex % 7;

    weeks[weekIndex].days[dayOfWeek] = {
      date: iso,
      count,
      level: getIntensityLevel(count, thresholds),
      dayOfWeek,
    };
  }

  return weeks;
}

export function getMonthBoundaries(weeks: HeatmapWeek[]): HeatmapMonthBoundary[] {
  const boundaries: HeatmapMonthBoundary[] = [];
  let lastMonth = -1;

  // A month starts at the week that contains its first day, wherever in the
  // week that day falls; scanning every day (not just the week's first) finds it.
  for (const week of weeks) {
    for (const day of week.days) {
      if (!day) continue;
      const month = parseInt(day.date.split("-")[1], 10) - 1;
      if (month !== lastMonth) {
        lastMonth = month;
        boundaries.push({ month, weekIndex: week.weekIndex });
      }
    }
  }

  return boundaries;
}

export function getHeatmapYearStats(days: DayCount[]): HeatmapYearStats {
  let totalEpisodes = 0;
  let bestDay: { date: string; count: number } | null = null;

  for (const d of days) {
    totalEpisodes += d.count;
    if (d.count > 0 && (!bestDay || d.count > bestDay.count)) {
      bestDay = { date: d.day, count: d.count };
    }
  }

  return { totalEpisodes, bestDay };
}
