import { useEffect, useState } from "react";
import { useLang, useT } from "../i18n";
import type { WatchInsights, WatchSummary, DustyEntry, BingeRecord, HourCount } from "../types";
import { BarChart, CategoryBlock, ShapeToggle } from "./StatsRings";
import { useStatsShape } from "../lib/statsShape";
import { useFormatNumber } from "../lib/formatNumber";
import { getDustyWatchlist, getBingeRecord, getHourlyDistribution, getAvgCompletionDays } from "../api";

// "Resumen" block for Estadísticas — local-only metrics computed by
// `get_watch_insights` (pure SQL, see src-tauri/src/db.rs). Sits between the
// existing scalar tiles and the Grafo/Barras selector (Stats.tsx), reusing
// StatsRings.tsx's bar/ring components so the screen never grows a third
// visual language. See
// docs/superpowers/specs/2026-07-13-stats-new-metrics-design.md.

const MINUTES_PER_DAY = 24 * 60;

// Rounds to whole hours first and only then splits into days + hours, so the
// two components can never disagree. Rounding the day remainder on its own
// could reach 24 (8630 min → 5d, remainder 1430 → round(1430/60) = 24) and
// render an impossible "5d 24h" instead of "6d 0h".
//
// Below an hour it reports minutes rather than a rounded-down "0h": a single
// externally-marked episode is ~24 minutes of real watch time, and showing
// zero next to a help line that says data exists reads as a bug.
function formatMinutes(t: ReturnType<typeof useT>, minutes: number): string {
  if (minutes < 60) {
    return t("stats.minutesUnit", { minutes: Math.round(minutes) });
  }
  const totalHours = Math.round(minutes / 60);
  if (minutes >= 2 * MINUTES_PER_DAY) {
    return t("stats.daysUnit", {
      days: Math.floor(totalHours / 24),
      hours: totalHours % 24,
    });
  }
  return t("stats.hoursUnit", { hours: totalHours });
}

// "2026-07-13" -> "13 jul" style short label for the activity axis, localized.
function axisDay(iso: string, locale: string): string {
  const d = new Date(`${iso}T00:00:00`);
  if (Number.isNaN(d.getTime())) return iso;
  return d.toLocaleDateString(locale, { day: "numeric", month: "short" });
}

// Compact 30-day activity chart: one thin vertical column per day in a single
// fixed-height row, instead of BarChart's 30 stacked full-width rows (which
// dominated the whole Stats page). Bars share one max, so height is
// comparable; a native title gives the exact date + count on hover, and the
// first/middle/last day are labelled so the axis reads without 30 tick labels.
function DayActivityChart({
  data,
  locale,
  emptyLabel,
}: {
  data: { day: string; count: number }[];
  locale: string;
  emptyLabel: string;
}) {
  const [grown, setGrown] = useState(false);
  useEffect(() => {
    const id = requestAnimationFrame(() => setGrown(true));
    return () => cancelAnimationFrame(id);
  }, []);
  const max = Math.max(1, ...data.map((d) => d.count));
  const total = data.reduce((s, d) => s + d.count, 0);
  const labelAt = new Set([0, Math.floor(data.length / 2), data.length - 1]);
  return (
    <div className="dayact">
      <div
        className="dayact-bars"
        role="img"
        aria-label={`${total} — ${emptyLabel}`}
      >
        {data.map((d, i) => {
          const pct = grown ? (d.count / max) * 100 : 0;
          return (
            <div
              className={`dayact-col${d.count > 0 ? " has" : ""}`}
              key={d.day}
              title={`${axisDay(d.day, locale)}: ${d.count}`}
            >
              <div className="dayact-fill" style={{ height: `${pct}%` }} />
              {labelAt.has(i) && <span className="dayact-tick">{axisDay(d.day, locale)}</span>}
            </div>
          );
        })}
      </div>
    </div>
  );
}

// Hourly distribution chart: 24 small vertical bars (0-23), inline height
// proportional to count/max. Data-driven inline style is expected here per
// codebase rule. Shows "daytime watcher" / "night owl" label based on peak hour.
function HourlyDistributionChart({
  data,
  t,
}: {
  data: HourCount[];
  t: ReturnType<typeof useT>;
}) {
  const [grown, setGrown] = useState(false);
  useEffect(() => {
    const id = requestAnimationFrame(() => setGrown(true));
    return () => cancelAnimationFrame(id);
  }, []);
  const max = Math.max(1, ...data.map((d) => d.count));
  const peakHour = data.reduce((a, b) => (a.count > b.count ? a : b)).hour;
  const isDaytime = peakHour >= 6 && peakHour <= 18;
  const label = isDaytime ? t("stats.hourlyDaytime") : t("stats.hourlyNight");

  return (
    <div className="hourly-dist">
      <div className="hourly-dist-bars" role="img" aria-label={`${data.reduce((s, d) => s + d.count, 0)} — ${label}`}>
        {data.map((d) => {
          const pct = grown ? (d.count / max) * 100 : 0;
          return (
            <div
              className={`hourly-col${d.count > 0 ? " has" : ""}`}
              key={d.hour}
              title={`${d.hour}:00 — ${d.count}`}
            >
              <div className="hourly-fill" style={{ height: `${pct}%` }} />
              <span className="hourly-tick">{d.hour}</span>
            </div>
          );
        })}
      </div>
      <div className="hourly-label">{label}</div>
    </div>
  );
}

export function StatsInsights({
  insights,
  summary,
}: {
  insights: WatchInsights;
  summary: WatchSummary;
}) {
  const t = useT();
  const n = useFormatNumber();
  const { lang } = useLang();
  const [shape, setShape] = useStatsShape();
  const [dusty, setDusty] = useState<DustyEntry[]>([]);

  useEffect(() => {
    getDustyWatchlist().then(setDusty).catch(() => setDusty([]));
  }, []);

  const [bingeRecord, setBingeRecord] = useState<BingeRecord | null>(null);

  useEffect(() => {
    getBingeRecord().then(setBingeRecord).catch(() => setBingeRecord({ day: null, count: 0 }));
  }, []);

  const [hourlyDist, setHourlyDist] = useState<HourCount[]>([]);

  useEffect(() => {
    getHourlyDistribution().then(setHourlyDist).catch(() => setHourlyDist([]));
  }, []);

  const [avgDays, setAvgDays] = useState<number | null>(null);

  useEffect(() => {
    getAvgCompletionDays().then(setAvgDays).catch(() => setAvgDays(null));
  }, []);

  const totalMinutes = insights.estimated_minutes_tracked + insights.estimated_minutes_external;
  const completionPct =
    summary.episodes_total > 0
      ? Math.round((summary.episodes_watched / summary.episodes_total) * 100)
      : 0;

  const topSeriesData = insights.top_series.map((s) => ({ name: s.title, count: s.count }));
  const funnelData = [
    { name: t("stats.funnelFollowed"), count: insights.followed_airing + insights.followed_finished },
    { name: t("stats.funnelWant"), count: insights.want },
    { name: t("stats.funnelDiscarded"), count: insights.discarded },
    { name: t("stats.funnelWatched"), count: insights.watched_externally },
  ];
  const airingVsFinishedData = [
    { name: t("stats.airing"), count: insights.followed_airing },
    { name: t("stats.finished"), count: insights.followed_finished },
  ];
  const hasMarks = insights.marks_by_day.some((d) => d.count > 0);
  const hasHourly = hourlyDist.some((d) => d.count > 0);

  return (
    <div className="stats-insights">
      <h3 className="section-title">{t("stats.insightsHeading")}</h3>

      <div className="stat-grid">
        <div className="stat-card">
          <div className="stat-label">{t("stats.timeWatched")}</div>
          <div className="stat-value">{formatMinutes(t, totalMinutes)}</div>
          <div className="stat-help">
            {t("stats.timeWatchedHelp", {
              done: n(insights.external_titles_estimated),
              total: n(insights.external_titles_total),
            })}
          </div>
        </div>
        <div className="stat-card">
          <div className="stat-label">{t("stats.completion")}</div>
          <div className="stat-value">{completionPct}%</div>
          <div
            className="progress"
            role="progressbar"
            aria-valuenow={completionPct}
            aria-valuemin={0}
            aria-valuemax={100}
          >
            <span style={{ width: `${completionPct}%` }} />
          </div>
          <div className="stat-help">
            {n(summary.episodes_watched)}/{n(summary.episodes_total)}
          </div>
        </div>
        <div className="stat-card">
          <div className="stat-label">{t("stats.avgEpisodes")}</div>
          <div className="stat-value">{insights.avg_episodes_per_series.toFixed(1)}</div>
        </div>
        <div className="stat-card">
          <div className="stat-label">{t("stats.bingeRecord")}</div>
          <div className="stat-value">
            {bingeRecord && bingeRecord.day
              ? t("stats.bingeRecordValue", { count: n(bingeRecord.count), day: axisDay(bingeRecord.day, lang) })
              : t("stats.bingeRecordEmpty")}
          </div>
        </div>
        <div className="stat-card">
          <div className="stat-label">{t("stats.completionSpeed")}</div>
          <div className="stat-value">
            {avgDays !== null ? (
              t("stats.completionSpeedValue", { days: avgDays.toFixed(1) })
            ) : (
              t("stats.completionSpeedEmpty")
            )}
          </div>
        </div>
      </div>

      {topSeriesData.length > 0 && (
        <div className="series-block">
          <div className="series-head">
            <h3 className="section-title">{t("stats.topSeries")}</h3>
          </div>
          <BarChart data={topSeriesData} />
        </div>
      )}

      <div className="section-toolbar">
        <ShapeToggle shape={shape} onChange={setShape} />
      </div>

      <div className="stats-cols">
        <div className="series-block">
          <div className="series-head">
            <h3 className="section-title">{t("stats.funnelHeading")}</h3>
          </div>
          <CategoryBlock data={funnelData} shape={shape} emptyMessage={t("stats.ringsEmpty")} />
        </div>

        <div className="series-block">
          <div className="series-head">
            <h3 className="section-title">{t("stats.airingVsFinished")}</h3>
          </div>
          <CategoryBlock
            data={airingVsFinishedData}
            shape={shape}
            emptyMessage={t("stats.ringsEmpty")}
          />
        </div>
      </div>

      {hasHourly && (
        <div className="series-block">
          <div className="series-head">
            <h3 className="section-title">{t("stats.hourlyHeading")}</h3>
          </div>
          <HourlyDistributionChart data={hourlyDist} t={t} />
        </div>
      )}

      {hasMarks && insights.marks_tracked_since && (
        <div className="series-block">
          <div className="series-head">
            <h3 className="section-title">{t("stats.marksHeading")}</h3>
          </div>
          <div className="dayact-wrap">
            <DayActivityChart
              data={insights.marks_by_day}
              locale={lang}
              emptyLabel={t("stats.marksHeading")}
            />
          </div>
          <div className="stats-caveat">
            {t("stats.marksCaveat", { date: insights.marks_tracked_since })}
          </div>
        </div>
      )}

      {dusty.length > 0 && (
        <div className="series-block">
          <div className="series-head">
            <h3 className="section-title">{t("stats.dustyHeading")}</h3>
          </div>
          <ul className="dusty-list">
            {dusty.map((entry) => (
              <li key={entry.title} className="dusty-item">
                <span className="dusty-title">{entry.title}</span>
                <span className="dusty-date">
                  {t("stats.dustyLastSeen", { date: entry.last_seen_at })}
                </span>
              </li>
            ))}
          </ul>
        </div>
      )}
    </div>
  );
}
