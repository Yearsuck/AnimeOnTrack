import { useEffect, useMemo, useRef, useState } from "react";
import { useLang, useT } from "../i18n";
import { useFormatNumber } from "../lib/formatNumber";
import type { YearlyActivity } from "../types";
import { getYearlyActivity, getActivityYears } from "../api";
import {
  buildHeatmapWeeks,
  getMonthBoundaries,
  getHeatmapYearStats,
} from "./stats/heatmapLayout";

function formatShortDate(iso: string, locale: string): string {
  const d = new Date(`${iso}T00:00:00`);
  if (Number.isNaN(d.getTime())) return iso;
  return d.toLocaleDateString(locale, { day: "numeric", month: "short" });
}

function formatFullDate(iso: string, locale: string): string {
  const d = new Date(`${iso}T00:00:00`);
  if (Number.isNaN(d.getTime())) return iso;
  return d.toLocaleDateString(locale, {
    weekday: "long",
    year: "numeric",
    month: "long",
    day: "numeric",
  });
}

/**
 * Full-year activity heatmap rendered with CSS Grid.
 * 7 rows (Mon–Sun) × up to 54 columns (weeks), 5 intensity levels.
 * Responsive, scalable with clamp, with scroll-snap on narrow screens.
 */
export function StatsHeatmap() {
  const t = useT();
  const n = useFormatNumber();
  const { lang } = useLang();
  const [years, setYears] = useState<number[] | null>(null);
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
        if (mounted) setError(String(e));
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
          setError(String(e));
          setLoading(false);
        }
      });
    return () => {
      mounted = false;
    };
  }, [selectedYear]);

  // Sort years ascending for natural prev/next navigation
  const sortedYears = useMemo(() => [...(years ?? [])].sort((a, b) => a - b), [years]);
  const currentIndex = selectedYear != null ? sortedYears.indexOf(selectedYear) : -1;

  const handlePrevYear = () => {
    if (currentIndex > 0) setSelectedYear(sortedYears[currentIndex - 1]);
  };

  const handleNextYear = () => {
    if (currentIndex >= 0 && currentIndex < sortedYears.length - 1) {
      setSelectedYear(sortedYears[currentIndex + 1]);
    }
  };

  const prevDisabled = currentIndex <= 0;
  const nextDisabled = currentIndex < 0 || currentIndex >= sortedYears.length - 1;

  const year = selectedYear ?? (activity?.year ?? new Date().getFullYear());

  const weeks = useMemo(() => {
    if (!activity) return [];
    return buildHeatmapWeeks(activity.year, activity.days);
  }, [activity]);

  // Cell size follows the container so the grid fills the row at any width
  // (a fixed size left a wide gap on the right in fullscreen). Clamped so
  // narrow windows keep readable cells and scroll instead of shrinking them.
  const scrollRef = useRef<HTMLDivElement>(null);
  const [cellSize, setCellSize] = useState<number | null>(null);
  const weekCount = weeks.length;
  useEffect(() => {
    const el = scrollRef.current;
    if (!el || weekCount === 0) return;
    const LABEL_GUTTER = 44; // weekday labels + gaps
    const GAP = 3;
    const measure = () => {
      const size = Math.floor((el.clientWidth - LABEL_GUTTER) / weekCount - GAP);
      setCellSize(Math.max(10, Math.min(28, size)));
    };
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(el);
    return () => observer.disconnect();
  }, [weekCount, activity]);

  const monthBoundaries = useMemo(() => {
    return getMonthBoundaries(weeks);
  }, [weeks]);

  const yearStats = useMemo(() => {
    if (!activity) return { totalEpisodes: 0, bestDay: null };
    return getHeatmapYearStats(activity.days);
  }, [activity]);

  const monthLabels = useMemo(() => {
    return monthBoundaries.map((b) => {
      const d = new Date(year, b.month, 1);
      return {
        month: b.month,
        weekIndex: b.weekIndex,
        label: d.toLocaleDateString(lang, { month: "short" }),
      };
    });
  }, [monthBoundaries, year, lang]);

  if (error) {
    return (
      <div className="heatmap-error" role="alert">
        {t("stats.heatmapError", { msg: error })}
      </div>
    );
  }

  if (years === null) {
    return (
      <div className="heatmap-empty" role="status">
        {t("stats.loading")}
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

  if (!activity || loading || activity.year !== selectedYear) {
    return (
      <div className="heatmap-empty" role="status">
        {loading ? t("stats.loading") : t("stats.heatmapNoData")}
      </div>
    );
  }

  return (
    <div className="stats-heatmap">
      {/* Header toolbar: one-line summary and year selector */}
      <div className="heatmap-toolbar">
        <div className="heatmap-summary-line">
          <span className="heatmap-summary-stat">
            {t("stats.heatmapYearSummary", { count: n(yearStats.totalEpisodes), year: String(year) })}
          </span>
          <span className="heatmap-summary-sep" aria-hidden="true">·</span>
          <span className="heatmap-summary-stat">
            {yearStats.bestDay
              ? t("stats.heatmapBestDay", {
                  date: formatShortDate(yearStats.bestDay.date, lang),
                  count: n(yearStats.bestDay.count),
                })
              : t("stats.heatmapNoActivity")}
          </span>
        </div>

        <div className="heatmap-year-control">
          <button
            type="button"
            className="btn btn-ghost heatmap-nav-btn"
            onClick={handlePrevYear}
            disabled={prevDisabled}
            aria-label={t("stats.heatmapPrevYear")}
            title={t("stats.heatmapPrevYear")}
          >
            ‹
          </button>
          <select
            id="heatmap-year"
            className="input heatmap-select"
            value={selectedYear ?? ""}
            onChange={(e) => setSelectedYear(Number(e.target.value))}
            aria-label={t("stats.heatmapYear")}
          >
            {sortedYears.map((y) => (
              <option key={y} value={y}>
                {y}
              </option>
            ))}
          </select>
          <button
            type="button"
            className="btn btn-ghost heatmap-nav-btn"
            onClick={handleNextYear}
            disabled={nextDisabled}
            aria-label={t("stats.heatmapNextYear")}
            title={t("stats.heatmapNextYear")}
          >
            ›
          </button>
        </div>
      </div>

      {/* Heatmap scroll wrapper: responsive down to 900px, scroll-snap below */}
      <div
        className="heatmap-scroll"
        ref={scrollRef}
        style={cellSize ? ({ "--cell-size": `${cellSize}px` } as React.CSSProperties) : undefined}
      >
        <div className="heatmap-layout-wrapper">
          {/* Weekday row labels (Mon, Wed, Fri) sticky on left */}
          <div className="heatmap-weekdays" aria-hidden="true">
            <span className="heatmap-weekday">{t("stats.heatmapMon")}</span>
            <span className="heatmap-weekday heatmap-weekday-empty" />
            <span className="heatmap-weekday">{t("stats.heatmapWed")}</span>
            <span className="heatmap-weekday heatmap-weekday-empty" />
            <span className="heatmap-weekday">{t("stats.heatmapFri")}</span>
            <span className="heatmap-weekday heatmap-weekday-empty" />
            <span className="heatmap-weekday heatmap-weekday-empty" />
          </div>

          <div className="heatmap-content">
            {/* Month labels header */}
            <div className="heatmap-months" aria-hidden="true">
              {monthLabels.map((m) => (
                <div
                  key={m.month}
                  className="heatmap-month"
                  style={{ gridColumnStart: m.weekIndex + 1 }}
                >
                  {m.label}
                </div>
              ))}
            </div>

            {/* CSS Grid for cells: 7 rows, auto columns, column flow */}
            <div
              className="heatmap-grid"
              role="grid"
              aria-label={t("stats.heatmapAria", { year })}
            >
              {weeks.map((week) =>
                week.days.map((day, dIdx) => {
                  if (!day) {
                    return (
                      <div
                        key={`empty-${week.weekIndex}-${dIdx}`}
                        className="heatmap-cell empty"
                        aria-hidden="true"
                      />
                    );
                  }
                  const label = `${formatFullDate(day.date, lang)}: ${n(day.count)} ${t("stats.heatmapEpisodes")}`;
                  return (
                    <div
                      key={day.date}
                      className={`heatmap-cell level-${day.level}`}
                      role="gridcell"
                      tabIndex={0}
                      title={label}
                      aria-label={label}
                    />
                  );
                })
              )}
            </div>
          </div>
        </div>
      </div>

      {/* Legend: Less -> More */}
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