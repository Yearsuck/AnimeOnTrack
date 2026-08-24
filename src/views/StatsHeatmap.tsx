import { useEffect, useState } from "react";
import { useLang, useT } from "../i18n";
import type { YearlyActivity } from "../types";
import { getYearlyActivity, getActivityYears } from "../api";

/**
 * GitHub-contributions-style full-year activity heatmap.
 * 7 rows (Mon–Sun) × ~53 columns (weeks), 5-level intensity.
 * Year selector defaults to most recent year with data.
 */
export function StatsHeatmap() {
  const t = useT();
  const { lang } = useLang();
  const [years, setYears] = useState<number[]>([]);
  const [selectedYear, setSelectedYear] = useState<number | null>(null);
  const [activity, setActivity] = useState<YearlyActivity | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Fetch available years on mount
  useEffect(() => {
    let mounted = true;
    getActivityYears()
      .then((y) => {
        if (mounted) {
          setYears(y);
          if (y.length > 0) setSelectedYear(y[0]);
        }
      })
      .catch((e) => {
        if (mounted) setError(e.toString());
      });
    return () => {
      mounted = false;
    };
  }, []);

  // Fetch activity when year changes
  useEffect(() => {
    if (selectedYear == null) return;
    let mounted = true;
    setLoading(true);
    setError(null);
    getYearlyActivity(selectedYear)
      .then((a) => {
        if (mounted) {
          setActivity(a);
          setLoading(false);
        }
      })
      .catch((e) => {
        if (mounted) {
          setError(e.toString());
          setLoading(false);
        }
      });
    return () => {
      mounted = false;
    };
  }, [selectedYear]);

  if (error) {
    return (
      <div className="heatmap-error">
        {t("stats.heatmapError", { msg: error })}
      </div>
    );
  }

  if (years.length === 0) {
    return (
      <div className="heatmap-empty">
        {t("stats.heatmapNoYears")}
      </div>
    );
  }

  if (!activity) {
    return (
      <div className="heatmap-empty">
        {loading ? t("common.loading") : t("stats.heatmapNoData")}
      </div>
    );
  }

  // Build a lookup map for quick count access
  const countMap = new Map<string, number>();
  activity.days.forEach((d) => countMap.set(d.day, d.count));

  // Determine the year's first day and total days (leap year handled by backend)
  const year = activity.year;
  const firstDay = new Date(`${year}-01-01T00:00:00`);
  const startOffset = (firstDay.getDay() + 6) % 7; // Monday=0
  const daysInYear = activity.days.length; // 365 or 366

  // Build grid cells: 7 rows × up to 53 columns
  const weeks = Math.ceil((startOffset + daysInYear) / 7);
  const cells: (string | null)[][] = Array.from({ length: 7 }, () =>
    Array.from({ length: weeks }, () => null)
  );

  let dayIndex = 0;
  for (let week = 0; week < weeks; week++) {
    for (let dayOfWeek = 0; dayOfWeek < 7; dayOfWeek++) {
      const cellIndex = week * 7 + dayOfWeek;
      if (cellIndex < startOffset) continue;
      if (dayIndex >= daysInYear) break;
      const iso = activity.days[dayIndex].day;
      cells[dayOfWeek][week] = iso;
      dayIndex++;
    }
  }

  // Day labels (Mon–Sun)
  const dayLabels = [
    t("stats.heatmapMon"),
    t("stats.heatmapTue"),
    t("stats.heatmapWed"),
    t("stats.heatmapThu"),
    t("stats.heatmapFri"),
    t("stats.heatmapSat"),
    t("stats.heatmapSun"),
  ];

  // Month boundaries for top labels
  const monthBounds: { month: number; week: number; label: string }[] = [];
  let lastMonth = -1;
  for (let week = 0; week < weeks; week++) {
    // Find first day in this week
    for (let dayOfWeek = 0; dayOfWeek < 7; dayOfWeek++) {
      const iso = cells[dayOfWeek][week];
      if (iso) {
        const month = parseInt(iso.split("-")[1], 10);
        if (month !== lastMonth) {
          lastMonth = month;
          const date = new Date(`${iso}T00:00:00`);
          monthBounds.push({
            month,
            week,
            label: date.toLocaleDateString(lang, { month: "short" }),
          });
        }
        break;
      }
    }
  }

  // Intensity level: 0 = none, 1-4 = increasing
  function intensityClass(count: number): string {
    if (count === 0) return "heatmap-cell level-0";
    if (count === 1) return "heatmap-cell level-1";
    if (count <= 3) return "heatmap-cell level-2";
    if (count <= 5) return "heatmap-cell level-3";
    return "heatmap-cell level-4";
  }

  return (
    <div className="stats-heatmap">
      <div className="heatmap-toolbar">
        <label className="heatmap-year-label" htmlFor="heatmap-year">
          {t("stats.heatmapYear")}
        </label>
        <select
          id="heatmap-year"
          className="lib-filter-select"
          value={selectedYear ?? ""}
          onChange={(e) => setSelectedYear(Number(e.target.value))}
        >
          {years.map((y) => (
            <option key={y} value={y}>
              {y}
            </option>
          ))}
        </select>
      </div>

      <div className="heatmap-grid-wrap">
        {/* Month labels row */}
        <div className="heatmap-months" role="row" aria-hidden="true">
          {monthBounds.map((m) => (
            <div
              key={m.month}
              className="heatmap-month"
              style={{ gridColumnStart: m.week + 1 }}
            >
              {m.label}
            </div>
          ))}
        </div>

        {/* Grid: 7 rows (days of week) */}
        <div className="heatmap-grid" role="img" aria-label={t("stats.heatmapAria", { year })}>
          {cells.map((weekCells, dayOfWeek) => (
            <div key={dayOfWeek} className="heatmap-row" role="row">
              <div className="heatmap-day-label" aria-hidden="true">
                {/* GitHub's own graph only labels every other row (Mon/Wed/Fri) —
                    a label per row is redundant clutter at 11px row height. */}
                {dayOfWeek % 2 === 0 ? dayLabels[dayOfWeek] : ""}
              </div>
              {weekCells.map((iso, week) => (
                <div
                  key={week}
                  className={
                    iso
                      ? intensityClass(countMap.get(iso) ?? 0)
                      : "heatmap-cell empty"
                  }
                  title={
                    iso
                      ? `${new Date(`${iso}T00:00:00`).toLocaleDateString(lang, {
                          weekday: "long",
                          year: "numeric",
                          month: "long",
                          day: "numeric",
                        })}: ${countMap.get(iso) ?? 0} ${t("stats.heatmapEpisodes")}`
                      : ""
                  }
                  role="cell"
                  aria-label={
                    iso
                      ? `${new Date(`${iso}T00:00:00`).toLocaleDateString(lang, {
                          weekday: "long",
                          year: "numeric",
                          month: "long",
                          day: "numeric",
                        })}: ${countMap.get(iso) ?? 0} ${t("stats.heatmapEpisodes")}`
                      : ""
                  }
                />
              ))}
            </div>
          ))}
        </div>
      </div>

      {/* Legend */}
      <div className="heatmap-legend" aria-hidden="true">
        <span className="heatmap-legend-label">{t("stats.heatmapLess")}</span>
        <div className="heatmap-legend-levels">
          <div className="heatmap-cell level-0" />
          <div className="heatmap-cell level-1" />
          <div className="heatmap-cell level-2" />
          <div className="heatmap-cell level-3" />
          <div className="heatmap-cell level-4" />
        </div>
        <span className="heatmap-legend-label">{t("stats.heatmapMore")}</span>
      </div>
    </div>
  );
}