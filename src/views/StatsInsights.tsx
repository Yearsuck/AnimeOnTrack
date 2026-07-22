import { useT } from "../i18n";
import type { WatchInsights, WatchSummary } from "../types";
import { BarChart, CategoryBlock, ShapeToggle } from "./StatsRings";
import { useStatsShape } from "../lib/statsShape";
import { useFormatNumber } from "../lib/formatNumber";

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

// "2026-07-13" -> "07-13", short enough for a bar-chart label without
// widening the shared 130px label column.
function shortDay(iso: string): string {
  return iso.length >= 10 ? iso.slice(5) : iso;
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
  const [shape, setShape] = useStatsShape();

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
  const marksData = insights.marks_by_day.map((d) => ({ name: shortDay(d.day), count: d.count }));

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

      {marksData.length > 0 && insights.marks_tracked_since && (
        <div className="series-block">
          <div className="series-head">
            <h3 className="section-title">{t("stats.marksHeading")}</h3>
          </div>
          <BarChart data={marksData} />
          <div className="stats-caveat">
            {t("stats.marksCaveat", { date: insights.marks_tracked_since })}
          </div>
        </div>
      )}
    </div>
  );
}
