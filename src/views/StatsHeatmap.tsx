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
        {/* Month labels, positioned by absolute pixel offset — 11px cell +
            3px gap per week column, matching the CSS below exactly. */}
        <div className="heatmap-months" aria-hidden="true">
          {monthBounds.map((m) => (
            <div key={m.month} className="heatmap-month" style={{ left: m.week * 14 }}>
              {m.label}
            </div>
          ))}
        </div>

        {/* Every label and cell is absolutely positioned by pixel math
            inside this one relative box — the same mechanism the month
            labels above already use correctly. Two nested flex levels
            (a flex row of per-week flex columns) turned out to detach a
            column from the row in WebView2; explicit left/top per cell
            leaves nothing for a browser layout quirk to get wrong. */}
        <div
          className="heatmap-grid"
          role="img"
          aria-label={t("stats.heatmapAria", { year })}
          style={{ width: 33 + weeks * 14 - 3, height: 7 * 14 - 3 }}
        >
          {dayLabels.map(
            (label, dayOfWeek) =>
              dayOfWeek % 2 === 0 && (
                // GitHub's own graph only labels every other row (Mon/Wed/Fri).
                <div
                  key={dayOfWeek}
                  className="heatmap-day-label"
                  style={{ top: dayOfWeek * 14 }}
                  aria-hidden="true"
                >
                  {label}
                </div>
              )
          )}
          {cells.flatMap((weekCells, dayOfWeek) =>
            weekCells.map((iso, week) => {
              const label = iso
                ? `${new Date(`${iso}T00:00:00`).toLocaleDateString(lang, {
                    weekday: "long",
                    year: "numeric",
                    month: "long",
                    day: "numeric",
                  })}: ${countMap.get(iso) ?? 0} ${t("stats.heatmapEpisodes")}`
                : "";
              return (
                <div
                  key={`${dayOfWeek}-${week}`}
                  style={{ left: 33 + week * 14, top: dayOfWeek * 14 }}
                  className={
                    iso ? intensityClass(countMap.get(iso) ?? 0) : "heatmap-cell empty"
                  }
                  title={label}
                  role="cell"
                  aria-label={label}
                />
              );
            })
          )}
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