import { useEffect, useMemo, useState } from "react";
import { useLang, useT } from "../i18n";
import type { WatchInsights, WatchSummary, DustyEntry, BingeRecord, HourCount, PopularityBias } from "../types";
import { BarChart, CategoryBlock, ShapeToggle } from "./StatsRings";
import { useStatsShape } from "../lib/statsShape";
import { useFormatNumber } from "../lib/formatNumber";
import { getDustyWatchlist, getBingeRecord, getHourlyDistribution, getAvgCompletionDays } from "../api";
import { StatsHeatmap } from "./StatsHeatmap";

const MINUTES_PER_DAY = 24 * 60;

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

function axisDay(iso: string, locale: string): string {
  const d = new Date(`${iso}T00:00:00`);
  if (Number.isNaN(d.getTime())) return iso;
  return d.toLocaleDateString(locale, { day: "numeric", month: "short" });
}

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
          const dayLabel = `${axisDay(d.day, locale)}: ${d.count}`;
          return (
            <div
              className={`dayact-col${d.count > 0 ? " has" : ""}`}
              key={d.day}
              tabIndex={0}
              role="img"
              aria-label={dayLabel}
              title={dayLabel}
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
  const peakHour = data.length > 0 ? data.reduce((a, b) => (a.count > b.count ? a : b)).hour : 0;
  const isDaytime = peakHour >= 6 && peakHour <= 18;
  const label = isDaytime ? t("stats.hourlyDaytime") : t("stats.hourlyNight");

  return (
    <div className="hourly-dist">
      <div className="hourly-dist-bars" role="img" aria-label={`${data.reduce((s, d) => s + d.count, 0)} — ${label}`}>
        {data.map((d) => {
          const pct = grown ? (d.count / max) * 100 : 0;
          const hourLabel = `${d.hour}:00 — ${d.count}`;
          return (
            <div
              className={`hourly-col${d.count > 0 ? " has" : ""}`}
              key={d.hour}
              tabIndex={0}
              role="img"
              aria-label={hourLabel}
              title={hourLabel}
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
  popularityBias,
}: {
  insights: WatchInsights;
  summary: WatchSummary;
  popularityBias: PopularityBias | null;
}) {
  const t = useT();
  const n = useFormatNumber();
  const { lang } = useLang();
  const [shape, setShape] = useStatsShape();
  const [activityTab, setActivityTab] = useState<"year" | "month">("year");
  const [dusty, setDusty] = useState<DustyEntry[]>([]);
  const [bingeRecord, setBingeRecord] = useState<BingeRecord | null>(null);
  const [hourlyDist, setHourlyDist] = useState<HourCount[] | null>(null);
  const [avgDays, setAvgDays] = useState<number | null>(null);

  // Parallel coordinated fetch: avoids waterfalls and cascading re-renders
  useEffect(() => {
    let mounted = true;
    Promise.all([
      getDustyWatchlist().catch(() => [] as DustyEntry[]),
      getBingeRecord().catch(() => ({ day: null, count: 0, top_series: [] } as BingeRecord)),
      getHourlyDistribution().catch(() => [] as HourCount[]),
      getAvgCompletionDays().catch(() => null),
    ]).then(([d, b, h, a]) => {
      if (!mounted) return;
      setDusty(d);
      setBingeRecord(b);
      setHourlyDist(h);
      setAvgDays(a);
    });
    return () => {
      mounted = false;
    };
  }, []);

  const totalMinutes = insights.estimated_minutes_tracked + insights.estimated_minutes_external;
  const completionPct =
    summary.episodes_total > 0
      ? Math.round((summary.episodes_watched / summary.episodes_total) * 100)
      : 0;

  const topSeriesData = useMemo(
    () => insights.top_series.map((s) => ({ name: s.title, count: s.count })),
    [insights.top_series]
  );

  const funnelData = useMemo(
    () => [
      { name: t("stats.funnelFollowed"), count: insights.followed_airing + insights.followed_finished },
      { name: t("stats.funnelWant"), count: insights.want },
      { name: t("stats.funnelDiscarded"), count: insights.discarded },
      { name: t("stats.funnelWatched"), count: insights.watched_externally },
    ],
    [insights.followed_airing, insights.followed_finished, insights.want, insights.discarded, insights.watched_externally, t]
  );

  const airingVsFinishedData = useMemo(
    () => [
      { name: t("stats.airing"), count: insights.followed_airing },
      { name: t("stats.finished"), count: insights.followed_finished },
    ],
    [insights.followed_airing, insights.followed_finished, t]
  );

  const hasMarks = insights.marks_by_day.some((d) => d.count > 0);
  const hasHourly = (hourlyDist ?? []).some((d) => d.count > 0);

  const popScore = popularityBias?.normalized_score ?? null;
  const popPct = popScore !== null ? Math.min(100, Math.max(0, popScore * 10)) : 0;

  return (
    <div className="stats-insights">
      {/* 1. KPI strip at the top: 4 headline numbers */}
      <section className="kpi-strip" aria-label={t("stats.insightsHeading")}>
        <div className="kpi-card">
          <div className="kpi-label">{t("stats.episodesWatched")}</div>
          <div className="kpi-value">
            {n(summary.episodes_watched + summary.episodes_watched_external)}
          </div>
          <div className="kpi-help">
            {t("stats.episodesWatchedHelp", {
              real: n(summary.episodes_watched),
              external: n(summary.episodes_watched_external),
            })}
          </div>
        </div>

        <div className="kpi-card">
          <div className="kpi-label">{t("stats.timeWatched")}</div>
          <div className="kpi-value">{formatMinutes(t, totalMinutes)}</div>
          <div className="kpi-help">
            {t("stats.timeWatchedHelp", {
              done: n(insights.external_titles_estimated),
              total: n(insights.external_titles_total),
            })}
          </div>
        </div>

        <div className="kpi-card">
          <div className="kpi-label">{t("stats.completion")}</div>
          <div className="kpi-value">{completionPct}%</div>
          <div
            className="progress"
            role="progressbar"
            aria-valuenow={completionPct}
            aria-valuemin={0}
            aria-valuemax={100}
            aria-label={t("stats.completion")}
          >
            <span style={{ width: `${completionPct}%` }} />
          </div>
          <div className="kpi-help">
            {n(summary.episodes_watched)} / {n(summary.episodes_total)}
          </div>
        </div>

        <div className="kpi-card">
          <div className="kpi-label">{t("stats.distinctAnime")}</div>
          <div className="kpi-value">{n(summary.distinct_anime)}</div>
          <div className="kpi-help">{t("stats.distinctAnimeHelp")}</div>
        </div>
      </section>

      {/* 2. Activity section: full-year heatmap and 30-day chart unified */}
      <section className="stats-section">
        <div className="stats-section-head">
          <h3 className="section-title">{t("stats.activitySection")}</h3>
          <div className="seg" role="tablist" aria-label={t("stats.activitySection")}>
            <button
              type="button"
              role="tab"
              aria-selected={activityTab === "year"}
              className={`seg-btn${activityTab === "year" ? " active" : ""}`}
              onClick={() => setActivityTab("year")}
            >
              {t("stats.activityTabYear")}
            </button>
            <button
              type="button"
              role="tab"
              aria-selected={activityTab === "month"}
              className={`seg-btn${activityTab === "month" ? " active" : ""}`}
              onClick={() => setActivityTab("month")}
            >
              {t("stats.activityTab30Days")}
            </button>
          </div>
        </div>

        <div className="stats-card">
          {activityTab === "year" ? (
            <StatsHeatmap />
          ) : (
            <div className="stats-30day-wrap">
              {hasMarks ? (
                <>
                  <div className="dayact-wrap">
                    <DayActivityChart
                      data={insights.marks_by_day}
                      locale={lang}
                      emptyLabel={t("stats.marksHeading")}
                    />
                  </div>
                  {insights.marks_tracked_since && (
                    <div className="stats-caveat">
                      {t("stats.marksCaveat", { date: insights.marks_tracked_since })}
                    </div>
                  )}
                </>
              ) : (
                <div className="empty">{t("stats.marksEmpty")}</div>
              )}
            </div>
          )}
        </div>
      </section>

      {/* 3. Rankings and insights in a responsive 2-column grid */}
      <div className="stats-grid-2col">
        {/* Column 1 */}
        <div className="stats-col">
          {/* Library and backlog status */}
          <section className="stats-card">
            <h4 className="stats-card-title">{t("stats.librarySection")}</h4>
            <div className="stats-mini-grid">
              <div className="stats-mini-stat">
                <span className="stats-mini-label">{t("stats.followedSeries")}</span>
                <span className="stats-mini-val">{n(summary.airing_followed)}</span>
              </div>
              <div className="stats-mini-stat">
                <span className="stats-mini-label">{t("stats.backlogPending")}</span>
                <span className="stats-mini-val">{n(summary.pending_to_watch)}</span>
                <span className="stats-mini-sub">{t("stats.backlogPendingHelp")}</span>
              </div>
              <div className="stats-mini-stat">
                <span className="stats-mini-label">{t("stats.wishlist")}</span>
                <span className="stats-mini-val">{n(summary.backlog_want)}</span>
                <span className="stats-mini-sub">{t("stats.wishlistHelp")}</span>
              </div>
              <div className="stats-mini-stat">
                <span className="stats-mini-label">{t("stats.backlog")}</span>
                <span className="stats-mini-val">
                  {(insights.backlog_episodes ?? 0) > 0 || (insights.backlog_minutes ?? 0) > 0
                    ? formatMinutes(t, insights.backlog_minutes ?? 0)
                    : t("stats.backlogEmpty")}
                </span>
                {(insights.backlog_episodes ?? 0) > 0 && (
                  <span className="stats-mini-sub">
                    {t(insights.backlog_episodes === 1 ? "stats.backlogEpisodesOne" : "stats.backlogEpisodes", {
                      n: n(insights.backlog_episodes),
                    })}
                  </span>
                )}
              </div>
            </div>
          </section>

          {/* Top Series */}
          <section className="stats-card">
            <h4 className="stats-card-title">{t("stats.topSeries")}</h4>
            {topSeriesData.length > 0 ? (
              <BarChart data={topSeriesData} />
            ) : (
              <div className="empty">{t("stats.topSeriesEmpty")}</div>
            )}
          </section>

          {/* Top Studios */}
          <section className="stats-card">
            <h4 className="stats-card-title">{t("stats.topStudios")}</h4>
            {insights.top_studios && insights.top_studios.length > 0 ? (
              <BarChart data={insights.top_studios.map((s) => ({ name: s.title, count: s.count }))} />
            ) : (
              <div className="empty">{t("stats.topStudiosEmpty")}</div>
            )}
          </section>

          {/* Discover Funnel */}
          <section className="stats-card">
            <div className="stats-card-head">
              <h4 className="stats-card-title">{t("stats.funnelHeading")}</h4>
              <ShapeToggle shape={shape} onChange={setShape} />
            </div>
            <CategoryBlock data={funnelData} shape={shape} emptyMessage={t("stats.funnelEmpty")} />
          </section>
        </div>

        {/* Column 2 */}
        <div className="stats-col">
          {/* Milestones and habits */}
          <section className="stats-card">
            <h4 className="stats-card-title">{t("stats.milestonesSection")}</h4>
            <div className="stats-mini-grid">
              <div className="stats-mini-stat">
                <span className="stats-mini-label">{t("stats.bingeRecord")}</span>
                <span className="stats-mini-val">
                  {bingeRecord && bingeRecord.day
                    ? t("stats.bingeRecordValue", { count: n(bingeRecord.count), day: axisDay(bingeRecord.day, lang) })
                    : t("stats.bingeRecordEmpty")}
                </span>
                {bingeRecord?.top_series && bingeRecord.top_series.length > 0 && (
                  <span className="stats-mini-sub">
                    {t("stats.bingeSeries", { series: bingeRecord.top_series.join(", ") })}
                  </span>
                )}
              </div>
              <div className="stats-mini-stat">
                <span className="stats-mini-label">{t("stats.completionSpeed")}</span>
                <span className="stats-mini-val">
                  {avgDays !== null ? (
                    t("stats.completionSpeedValue", { days: avgDays.toFixed(1) })
                  ) : (
                    t("stats.completionSpeedEmpty")
                  )}
                </span>
              </div>
              <div className="stats-mini-stat">
                <span className="stats-mini-label">{t("stats.avgEpisodes")}</span>
                <span className="stats-mini-val">{insights.avg_episodes_per_series.toFixed(1)}</span>
              </div>
            </div>
          </section>

          {/* Era Distribution */}
          <section className="stats-card">
            <h4 className="stats-card-title">{t("stats.eraDistribution")}</h4>
            {insights.era_distribution && insights.era_distribution.length > 0 ? (
              <BarChart data={insights.era_distribution.map((e) => ({ name: t("stats.eraDecade", { decade: e.decade }), count: e.count }))} />
            ) : (
              <div className="empty">{t("stats.eraEmpty")}</div>
            )}
          </section>

          {/* Airing vs Finished */}
          <section className="stats-card">
            <h4 className="stats-card-title">{t("stats.airingVsFinished")}</h4>
            <CategoryBlock
              data={airingVsFinishedData}
              shape={shape}
              emptyMessage={t("stats.ringsEmpty")}
            />
          </section>

          {/* 24-Hour distribution */}
          <section className="stats-card">
            <h4 className="stats-card-title">{t("stats.hourlyHeading")}</h4>
            {hourlyDist === null ? null : hasHourly ? (
              <HourlyDistributionChart data={hourlyDist} t={t} />
            ) : (
              <div className="empty">{t("stats.hourlyEmpty")}</div>
            )}
          </section>

          {/* Analytical Popularity Index (redesigned, no fake slider thumb) */}
          {popularityBias && (
            <section className="stats-card">
              <h4 className="stats-card-title">{t("stats.popularityBiasHeading")}</h4>
              <div className="popbias">
                <div className="popbias-labels">
                  <span className="popbias-low">{t("stats.popularityBiasLow")}</span>
                  <span className="popbias-score">
                    {popScore !== null ? `${popScore.toFixed(1)} / 10` : "—"}
                  </span>
                  <span className="popbias-high">{t("stats.popularityBiasHigh")}</span>
                </div>
                <div
                  className="popbias-meter"
                  role="meter"
                  aria-valuemin={0}
                  aria-valuemax={10}
                  aria-valuenow={popScore !== null ? Math.round(popScore * 10) / 10 : 0}
                  aria-label={t("stats.popularityBiasLabel")}
                >
                  <div className="popbias-track">
                    <div
                      className="popbias-fill"
                      style={{ width: `${popPct}%` }}
                    />
                    <div
                      className="popbias-marker"
                      style={{ left: `${popPct}%` }}
                    />
                  </div>
                </div>
                <div className="popbias-sub">{t("stats.popularityBiasLabel")}</div>
                <div className="stats-caveat">{t("stats.popularityBiasHelp")}</div>
              </div>
            </section>
          )}

          {/* Dusty Watchlist */}
          {dusty.length > 0 && (
            <section className="stats-card">
              <h4 className="stats-card-title">{t("stats.dustyHeading")}</h4>
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
            </section>
          )}
        </div>
      </div>
    </div>
  );
}
