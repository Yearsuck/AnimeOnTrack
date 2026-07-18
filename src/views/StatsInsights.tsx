import { useT } from "../i18n";
import type { WatchInsights, WatchSummary } from "../types";
import { BarChart, CategoryBlock, ShapeToggle } from "./StatsRings";
import { useStatsShape } from "../lib/statsShape";

// "Resumen" block for Estadísticas — local-only metrics computed by
// `get_watch_insights` (pure SQL, see src-tauri/src/db.rs). Sits between the
// existing scalar tiles and the Grafo/Barras selector (Stats.tsx), reusing
// StatsRings.tsx's bar/ring components so the screen never grows a third
// visual language. See
// docs/superpowers/specs/2026-07-13-stats-new-metrics-design.md.

const MINUTES_PER_DAY = 24 * 60;

function formatMinutes(t: ReturnType<typeof useT>, minutes: number): string {
  if (minutes >= 2 * MINUTES_PER_DAY) {
    const days = Math.floor(minutes / MINUTES_PER_DAY);
    const hours = Math.round((minutes % MINUTES_PER_DAY) / 60);
    return t("stats.daysUnit", { days, hours });
  }
  return t("stats.hoursUnit", { hours: Math.round(minutes / 60) });
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
    <div style={{ marginBottom: 28 }}>
      <h3 className="card-title" style={{ marginBottom: 12 }}>
        {t("stats.insightsHeading")}
      </h3>

      <div className="grid" style={{ marginBottom: 20 }}>
        <div className="card">
          <div className="card-body">
            <div className="muted" style={{ fontSize: 12 }}>
              {t("stats.timeWatched")}
            </div>
            <div style={{ fontSize: 22, fontWeight: 700 }}>{formatMinutes(t, totalMinutes)}</div>
            <div className="muted" style={{ fontSize: 11, marginTop: 2 }}>
              {t("stats.timeWatchedHelp", {
                done: insights.external_titles_estimated,
                total: insights.external_titles_total,
              })}
            </div>
          </div>
        </div>
        <div className="card">
          <div className="card-body">
            <div className="muted" style={{ fontSize: 12 }}>
              {t("stats.completion")}
            </div>
            <div style={{ fontSize: 22, fontWeight: 700 }}>{completionPct}%</div>
            <div className="progress" style={{ marginTop: 8 }}>
              <span style={{ width: `${completionPct}%` }} />
            </div>
            <div className="muted" style={{ fontSize: 11, marginTop: 4 }}>
              {summary.episodes_watched}/{summary.episodes_total}
            </div>
          </div>
        </div>
        <div className="card">
          <div className="card-body">
            <div className="muted" style={{ fontSize: 12 }}>
              {t("stats.avgEpisodes")}
            </div>
            <div style={{ fontSize: 22, fontWeight: 700 }}>
              {insights.avg_episodes_per_series.toFixed(1)}
            </div>
          </div>
        </div>
      </div>

      {topSeriesData.length > 0 && (
        <div className="series-block" style={{ marginBottom: 20 }}>
          <div className="series-head">
            <h3 className="card-title">{t("stats.topSeries")}</h3>
          </div>
          <BarChart data={topSeriesData} />
        </div>
      )}

      <div style={{ marginBottom: 12 }}>
        <ShapeToggle shape={shape} onChange={setShape} />
      </div>

      <div className="stats-cols" style={{ marginBottom: 20 }}>
        <div className="series-block" style={{ marginBottom: 0 }}>
          <div className="series-head">
            <h3 className="card-title">{t("stats.funnelHeading")}</h3>
          </div>
          <CategoryBlock data={funnelData} shape={shape} emptyMessage={t("stats.ringsEmpty")} />
        </div>

        <div className="series-block" style={{ marginBottom: 0 }}>
          <div className="series-head">
            <h3 className="card-title">{t("stats.airingVsFinished")}</h3>
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
            <h3 className="card-title">{t("stats.marksHeading")}</h3>
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
