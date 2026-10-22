import { listen } from "@tauri-apps/api/event";
import { useEffect, useMemo, useRef, useState } from "react";
import {
  cancelDownload,
  deleteDownload,
  deleteSeriesDownloads,
  downloadEpisode,
  downloadPending,
  getAnimeExtras,
  getCatalogInfoForSeries,
  getNextAir,
  getPlayerSettings,
  getSeriesSiteInfo,
  linkCatalogSeries,
  listEpisodes,
  openEpisode,
  playDownload,
  reclassifySeries,
  setPlayerSettings,
  setSeenCascade,
} from "../api";

import { useT, type MessageKey } from "../i18n";
import { airCountdown } from "../lib/airCountdown";
import { aniListTarget, seasonOf, sitePageUrl } from "../lib/anilistLinks";
import { isUnlinkedCatalogRow } from "../lib/catalogLink";
import { summarizeDownloads } from "../lib/downloadSummary";
import { epNum } from "../lib/episodeNumber";
import { resolveCoverUrl } from "../lib/posterFallback";
import type { AnimeExtras, CatalogAnime, Episode, NextAir, Series, SeriesSiteInfo } from "../types";
import { RelatedRow, SeriesAbout } from "./SeriesInfo";

const STATUS_KEYS: Record<string, MessageKey> = {
  RELEASING: "seriesDetail.status.RELEASING",
  FINISHED: "seriesDetail.status.FINISHED",
  NOT_YET_RELEASED: "seriesDetail.status.NOT_YET_RELEASED",
  CANCELLED: "seriesDetail.status.CANCELLED",
  HIATUS: "seriesDetail.status.HIATUS",
};

const SEASON_KEYS: Record<"winter" | "spring" | "summer" | "fall", MessageKey> = {
  winter: "seriesDetail.season.winter",
  spring: "seriesDetail.season.spring",
  summer: "seriesDetail.season.summer",
  fall: "seriesDetail.season.fall",
};

export function SeriesDetail({
  series,
  onBack,
  onChanged,
  onOpenSeries,
}: {
  series: Series;
  onBack: () => void;
  onChanged: () => void;
  /** Open another series (a related title the user already has a row for). */
  onOpenSeries?: (series: Series) => void;
}) {
  const t = useT();
  const [episodes, setEpisodes] = useState<Episode[]>([]);
  const [loading, setLoading] = useState(true);
  const [linking, setLinking] = useState(false);
  // Anime metadata (genres/studio/format/episodes/score/cover/AniList url)
  // comes from AniList, not the scraped site — the site only still supplies
  // episode links and the "is it airing" signal (see `listEpisodes` below).
  const [catalogInfo, setCatalogInfo] = useState<CatalogAnime | null>(null);
  const [catalogLoaded, setCatalogLoaded] = useState(false);
  const [siteInfo, setSiteInfo] = useState<SeriesSiteInfo | null>(null);
  const [extras, setExtras] = useState<AnimeExtras | null>(null);
  const [bannerFailed, setBannerFailed] = useState<string | null>(null);
  const topRef = useRef<HTMLDivElement>(null);
  const [nextAir, setNextAir] = useState<NextAir | null>(null);
  // Live state of downloads started or running while this screen is open
  // (`download-progress` events); the episode row's own `download_status`
  // covers everything else, e.g. a finished download from an earlier visit.
  const [dl, setDl] = useState<Record<number, { percent: number | null; error?: string }>>({});
  const [batchRunning, setBatchRunning] = useState(false);
  const [panelMsg, setPanelMsg] = useState<string | null>(null);
  const [confirmDeleteAll, setConfirmDeleteAll] = useState(false);
  const [deletingAll, setDeletingAll] = useState(false);

  useEffect(() => {
    setConfirmDeleteAll(false);
    setPanelMsg(null);
  }, [series.id]);

  // AniList's CDN cover wins; otherwise the series' own (possibly a cached
  // local `file:` path, which the raw <img> would be blocked from loading).
  const detailCover = resolveCoverUrl(catalogInfo?.cover_url ?? series.cover_url);
  const rowRefs = useRef<Map<number, HTMLDivElement>>(new Map());
  // Guards the link-on-open trigger against React StrictMode's dev-only
  // double-invoke (same pattern as App.tsx's startup effect) — otherwise
  // opening an unlinked catalog row would fire two scrapes.
  const linkTriedRef = useRef<number | null>(null);

  const [playerFullscreen, setPlayerFullscreen] = useState(false);

  useEffect(() => {
    getPlayerSettings()
      .then((s) => setPlayerFullscreen(s.fullscreen))
      .catch(() => {});
  }, []);

  const togglePlayerFullscreen = async () => {
    const next = !playerFullscreen;
    setPlayerFullscreen(next);
    try {
      const current = await getPlayerSettings();
      await setPlayerSettings(current.monitor, next);
    } catch (err) {
      console.error("Failed to update player fullscreen:", err);
    }
  };


  async function load(): Promise<Episode[]> {
    setLoading(true);
    try {
      const eps = await listEpisodes(series.id);
      setEpisodes(eps);
      return eps;
    } finally {
      setLoading(false);
    }
  }
  // `catalogLoaded` gates the AniList link: until the lookup answers there is
  // no AniList URL to open, and the series' own (site) URL must never stand in
  // for it under an AniList label.
  useEffect(() => {
    let cancelled = false;
    setCatalogInfo(null);
    setCatalogLoaded(false);
    getCatalogInfoForSeries(series.id)
      .then((info) => {
        if (!cancelled) setCatalogInfo(info);
      })
      .catch(() => {
        if (!cancelled) setCatalogInfo(null);
      })
      .finally(() => {
        if (!cancelled) setCatalogLoaded(true);
      });
    return () => {
      cancelled = true;
    };
  }, [series.id]);

  useEffect(() => {
    let cancelled = false;
    setSiteInfo(null);
    // Opening a related title swaps the series in place: start at the top.
    topRef.current?.scrollIntoView({ block: "start" });
    getSeriesSiteInfo(series.id)
      .then((info) => {
        if (!cancelled) setSiteInfo(info);
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, [series.id]);

  // AniList description + relations: cached in the DB, fetched once on demand.
  useEffect(() => {
    let cancelled = false;
    setExtras(null);
    if (!catalogInfo?.id) return;
    getAnimeExtras(catalogInfo.id)
      .then((res) => {
        if (!cancelled) setExtras(res);
      })
      .catch((err) => {
        console.error("getAnimeExtras failed", err);
        if (!cancelled) setExtras({ description: null, relations: [] });
      });
    return () => {
      cancelled = true;
    };
  }, [catalogInfo?.id]);

  useEffect(() => {
    let cancelled = false;
    if (!catalogInfo?.id) {
      setNextAir(null);
      return;
    }
    getNextAir(catalogInfo.id)
      .then((res) => {
        if (!cancelled) setNextAir(res);
      })
      .catch(() => {
        if (!cancelled) setNextAir(null);
      });
    return () => {
      cancelled = true;
    };
  }, [catalogInfo?.id]);

  const lastDlUpdateRef = useRef<Map<number, number>>(new Map());
  const trailingDlTimersRef = useRef<Map<number, ReturnType<typeof setTimeout>>>(new Map());

  useEffect(() => {
    const THROTTLE_MS = 250; // ~4 per second per episode
    // `unlisten` resolves asynchronously: an event delivered after cleanup
    // must not schedule a timer that nothing would clear.
    let cancelled = false;
    const unlisten = listen<{
      episode_id: number;
      status: "downloading" | "downloaded" | "failed" | "cancelled";
      percent: number | null;
      error: string | null;
    }>("download-progress", (e) => {
      if (cancelled) return;
      const p = e.payload;
      const isTerminal = p.status !== "downloading";

      const existingTimer = trailingDlTimersRef.current.get(p.episode_id);
      if (existingTimer) {
        clearTimeout(existingTimer);
        trailingDlTimersRef.current.delete(p.episode_id);
      }

      if (isTerminal) {
        lastDlUpdateRef.current.delete(p.episode_id);
        setDl((prev) => ({
          ...prev,
          [p.episode_id]: { percent: p.percent, error: p.error ?? undefined },
        }));
        load().catch(() => {});
        return;
      }

      const now = Date.now();
      const last = lastDlUpdateRef.current.get(p.episode_id) ?? 0;
      const elapsed = now - last;

      if (elapsed >= THROTTLE_MS) {
        lastDlUpdateRef.current.set(p.episode_id, now);
        setDl((prev) => ({
          ...prev,
          [p.episode_id]: { percent: p.percent, error: p.error ?? undefined },
        }));
      } else {
        const timer = setTimeout(() => {
          lastDlUpdateRef.current.set(p.episode_id, Date.now());
          trailingDlTimersRef.current.delete(p.episode_id);
          setDl((prev) => ({
            ...prev,
            [p.episode_id]: { percent: p.percent, error: p.error ?? undefined },
          }));
        }, THROTTLE_MS - elapsed);
        trailingDlTimersRef.current.set(p.episode_id, timer);
      }
    });
    return () => {
      cancelled = true;
      unlisten.then((f) => f()).catch(() => {});
      for (const timer of trailingDlTimersRef.current.values()) {
        clearTimeout(timer);
      }
      trailingDlTimersRef.current.clear();
      lastDlUpdateRef.current.clear();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [series.id]);

  useEffect(() => {
    linkTriedRef.current = null;
    (async () => {
      const eps = await load();
      // Opening the detail view of an unlinked catalog row (synthetic
      // `anilist-{id}` slug, no episodes yet) is one of the three explicit
      // link triggers in the design spec: the user asked to see episode
      // titles, so scrape the site for this one title, then reload. Only
      // when there are genuinely no episodes and it's still an unlinked
      // catalog row — a real site series with no episodes must not scrape.
      if (eps.length === 0 && isUnlinkedCatalogRow(series) && linkTriedRef.current !== series.id) {
        linkTriedRef.current = series.id;
        setLinking(true);
        try {
          await linkCatalogSeries(series.id);
        } catch (err) {
          console.error("linkCatalogSeries failed for", series.id, err);
        } finally {
          setLinking(false);
        }
        await load();
        onChanged();
      }
    })();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [series.id]);

  // Cascading + optimistic: update local state immediately (no reload, so
  // scroll position never jumps), then persist in the background. Marking an
  // episode seen also marks every earlier one seen; marking unseen also
  // un-marks every later one — watching stays gap-free.
  function toggleSeen(ep: Episode) {
    const n = epNum(ep.number);
    const nextSeen = !ep.seen;
    setEpisodes((prev) =>
      prev.map((e) => {
        if (n === null) return e.number === ep.number ? { ...e, seen: nextSeen } : e;
        const en = epNum(e.number);
        if (en === null) return e;
        if (nextSeen && en <= n) return { ...e, seen: true };
        if (!nextSeen && en >= n) return { ...e, seen: false };
        return e;
      })
    );
    setSeenCascade(series.id, ep.number, nextSeen)
      .catch((err) => {
        console.error("setSeenCascade failed for episode", ep.number, err);
        return load().catch(() => {});
      })
      .finally(onChanged);
  }

  // Local/instant, no scrape — the universal reclassify inverse (see
  // docs/superpowers/specs/2026-07-11-reversibility-classifications-design.md).
  // Re-following later goes through the existing Airing "+ Seguir" toggle,
  // which still finds the episode rows intact (reclassify never touches
  // episodes/seen).
  async function unfollow() {
    await reclassifySeries(series.id, "None");
    onChanged();
    onBack();
  }

  function jumpToCurrent() {
    const firstUnseen = episodes.find((e) => !e.seen);
    const target = firstUnseen ?? episodes[episodes.length - 1];
    if (target) {
      rowRefs.current.get(target.id)?.scrollIntoView({ behavior: "smooth", block: "center" });
    }
  }

  const seenCount = episodes.filter((e) => e.seen).length;
  const pct = episodes.length ? Math.round((seenCount / episodes.length) * 100) : 0;
  const countdown =
    nextAir != null ? airCountdown(nextAir.air_at, Date.now() / 1000) : null;
  const when = countdown
    ? t(
        countdown.unit === "minutes"
          ? "seriesDetail.nextAir.minutes"
          : countdown.unit === "hours"
          ? "seriesDetail.nextAir.hours"
          : "seriesDetail.nextAir.days",
        { n: countdown.value }
      )
    : "";

  const aniList = aniListTarget(catalogLoaded, catalogInfo?.url, series.title);
  const sitePage = sitePageUrl(series.url);
  const banner =
    catalogInfo?.banner_url && catalogInfo.banner_url !== bannerFailed ? catalogInfo.banner_url : null;
  // The site's own title when it differs from AniList's (often the Spanish
  // or romaji name the user knows the show by).
  const altTitle =
    catalogInfo && catalogInfo.title.trim().toLowerCase() !== series.title.trim().toLowerCase()
      ? series.title
      : null;

  const facts = useMemo(() => {
    if (!catalogInfo) return [];
    const out: { label: string; value: string }[] = [];
    const statusKey = catalogInfo.status ? STATUS_KEYS[catalogInfo.status] : undefined;
    if (statusKey) out.push({ label: t("seriesDetail.info.status"), value: t(statusKey) });
    const season = seasonOf(catalogInfo.start_date);
    if (season) {
      out.push({
        label: t("seriesDetail.info.season"),
        value: t(SEASON_KEYS[season.season], { year: season.year }),
      });
    }
    if (catalogInfo.format) out.push({ label: t("seriesDetail.info.format"), value: catalogInfo.format });
    if (catalogInfo.episodes != null) {
      out.push({ label: t("seriesDetail.info.episodes"), value: String(catalogInfo.episodes) });
    }
    if (catalogInfo.duration != null && catalogInfo.duration > 0) {
      out.push({
        label: t("seriesDetail.info.duration"),
        value: t("seriesDetail.info.durationValue", { n: catalogInfo.duration }),
      });
    }
    if (catalogInfo.studio) out.push({ label: t("seriesDetail.info.studio"), value: catalogInfo.studio });
    if (catalogInfo.average_score != null) {
      out.push({ label: t("seriesDetail.info.score"), value: `${catalogInfo.average_score}/100` });
    }
    return out;
  }, [catalogInfo, t]);

  // Opens inside an app-owned WebView2 window instead of the system browser.
  function openExternal(e: React.MouseEvent, url: string) {
    e.preventDefault();
    openEpisode(url).catch((err) => console.error("openEpisode failed for", url, err));
  }

  const summary = useMemo(() => summarizeDownloads(episodes), [episodes]);

  const panelStatus = useMemo(() => {
    const parts: string[] = [];
    if (summary.downloaded > 0) {
      parts.push(t("download.statusDownloaded", { count: summary.downloaded }));
    }
    if (summary.active > 0) {
      parts.push(t("download.statusActive", { count: summary.active }));
    }
    if (summary.failed > 0) {
      parts.push(t("download.statusFailed", { count: summary.failed }));
    }
    return parts.length > 0 ? parts.join(" · ") : t("download.panelEmpty");
  }, [summary.downloaded, summary.active, summary.failed, t]);

  async function handleDownloadPending() {
    setBatchRunning(true);
    setPanelMsg(null);
    try {
      const res = await downloadPending(series.id);
      await load();
      let msg = t("download.batchQueued", { count: res.queued });
      if (res.unsupported > 0) {
        msg += ` ${t("download.batchUnsupported", { count: res.unsupported })}`;
      }
      setPanelMsg(msg);
    } catch (err) {
      console.error("downloadPending failed", err);
      setPanelMsg(t("download.batchError"));
    } finally {
      setBatchRunning(false);
    }
  }

  async function handleDeleteAll() {
    setDeletingAll(true);
    try {
      const count = await deleteSeriesDownloads(series.id);
      await load();
      setPanelMsg(t("download.deleteAllDone", { count }));
    } catch (err) {
      console.error("deleteSeriesDownloads failed", err);
    } finally {
      setDeletingAll(false);
      setConfirmDeleteAll(false);
    }
  }

  return (
    <div className="page" ref={topRef}>
      <button className="btn btn-ghost detail-back" onClick={onBack}>
        {t("common.back")}
      </button>

      <header className={`detail-hero${banner ? " has-banner" : ""}`}>
        {banner && (
          <img
            className="detail-banner"
            src={banner}
            alt=""
            aria-hidden="true"
            onError={() => setBannerFailed(banner)}
          />
        )}
        <div className="detail-hero-grid">
          {detailCover && <img className="detail-cover" src={detailCover} alt="" />}
          <div className="detail-main">
            <h2 className="page-title detail-title">{catalogInfo?.title ?? series.title}</h2>
            {altTitle && <p className="muted detail-alt-title">{altTitle}</p>}
            <div className="detail-links">
              {/* Only ever anilist.co under an AniList label; nothing while the
                  catalog lookup is still running. */}
              {aniList.kind === "open" && (
                <a href={aniList.url} onClick={(e) => openExternal(e, aniList.url)}>
                  {t("seriesDetail.openPage")}
                </a>
              )}
              {aniList.kind === "search" && (
                <a
                  href={aniList.url}
                  title={t("seriesDetail.searchAniListHint")}
                  onClick={(e) => openExternal(e, aniList.url)}
                >
                  {t("seriesDetail.searchAniList")}
                </a>
              )}
              {sitePage && (
                <a className="detail-site-link" href={sitePage} onClick={(e) => openExternal(e, sitePage)}>
                  {siteInfo?.site_name
                    ? t("seriesDetail.openSite", { site: siteInfo.site_name })
                    : t("seriesDetail.openSiteGeneric")}
                </a>
              )}
            </div>
            {catalogInfo ? (
              facts.length > 0 && (
                <dl className="detail-facts">
                  {facts.map((f) => (
                    <div key={f.label} className="detail-fact">
                      <dt>{f.label}</dt>
                      <dd>{f.value}</dd>
                    </div>
                  ))}
                </dl>
              )
            ) : (
              catalogLoaded && <div className="muted detail-meta">{t("seriesDetail.info.notSynced")}</div>
            )}
            {nextAir && countdown && (
              <p className="detail-next-air">
                {t("seriesDetail.nextAir", { episode: nextAir.episode, when })}
              </p>
            )}
            {catalogInfo != null && catalogInfo.genres.length > 0 && (
              <div className="detail-genres">
                {catalogInfo.genres.map((g) => (
                  <span key={g} className="tag">
                    {g}
                  </span>
                ))}
              </div>
            )}
            <div className="detail-progress">
              <div className="muted detail-progress-label">
                {t("seriesDetail.seenCount", { seen: seenCount, total: episodes.length })}
              </div>
              <div className="progress">
                <span style={{ width: `${pct}%` }} />
              </div>
            </div>
            <div className="detail-actions">
              {episodes.length > 0 && (
                <button className="btn" onClick={jumpToCurrent}>
                  {t("seriesDetail.jumpToCurrent")}
                </button>
              )}
              {episodes.length > 0 && (
                <button
                  className={`btn ${playerFullscreen ? "btn-primary" : "btn-ghost"}`}
                  onClick={togglePlayerFullscreen}
                  title={t("settings.playerHelp")}
                >
                  {playerFullscreen ? `⛶ ${t("settings.playerFullscreen")}` : `🗗 ${t("settings.playerWindowed")}`}
                </button>
              )}
              {series.followed && (
                <button className="btn btn-ghost" onClick={unfollow}>
                  {t("seriesDetail.unfollow")}
                </button>
              )}
            </div>
          </div>
          <SeriesAbout
            key={series.id}
            siteSynopsis={siteInfo?.synopsis ?? null}
            siteName={siteInfo?.site_name ?? null}
            anilistDescription={extras?.description ?? null}
            anilistLoading={!catalogLoaded || (catalogInfo != null && extras == null)}
            tags={catalogInfo?.tags ?? []}
          />
        </div>
      </header>

      <RelatedRow items={extras?.relations ?? []} onOpenSeries={onOpenSeries} />

      {summary.downloadable > 0 && (
        <section className="dl-panel">
          <div className="dl-panel-head">
            <h3 className="section-title">{t("download.panelTitle")}</h3>
            <span className="dl-panel-status">{panelStatus}</span>
          </div>

          <div className="dl-panel-actions">
            <button
              type="button"
              className="btn btn-primary"
              disabled={batchRunning || summary.pending === 0}
              title={summary.pending === 0 ? t("download.nothingPending") : undefined}
              onClick={handleDownloadPending}
            >
              {t("download.downloadPending", { count: summary.pending })}
            </button>

            {summary.downloaded + summary.active + summary.failed > 0 && (
              confirmDeleteAll ? (
                <>
                  <button
                    type="button"
                    className="btn btn-danger"
                    disabled={deletingAll}
                    onClick={handleDeleteAll}
                  >
                    {t("download.deleteAllYes")}
                  </button>
                  <button
                    type="button"
                    className="btn btn-ghost"
                    disabled={deletingAll}
                    onClick={() => setConfirmDeleteAll(false)}
                  >
                    {t("download.deleteAllNo")}
                  </button>
                </>
              ) : (
                <button
                  type="button"
                  className="btn"
                  onClick={() => setConfirmDeleteAll(true)}
                >
                  {t("download.deleteAll", { count: summary.downloaded + summary.active })}
                </button>
              )
            )}
          </div>

          <p className="dl-panel-hint">
            {t("download.keepWatchedHint")}
          </p>

          {panelMsg && (
            <p className="dl-panel-msg">{panelMsg}</p>
          )}
        </section>
      )}

      {linking ? (
        <div className="empty">{t("seriesDetail.linking")}</div>
      ) : loading ? (
        <div className="empty">{t("seriesDetail.loadingEpisodes")}</div>
      ) : episodes.length === 0 ? (
        <div className="empty">{t("seriesDetail.noEpisodes")}</div>
      ) : (
        <div className="series-block">
          {episodes.map((ep) => {
            const isDownloading = ep.download_status === "downloading";
            const isDownloaded = ep.download_status === "downloaded";
            const rowClass = [
              "ep-row",
              ep.seen ? "seen" : "",
              isDownloading ? "dl-row-active" : "",
              isDownloaded ? "dl-row-done" : "",
            ]
              .filter(Boolean)
              .join(" ");

            return (
              <div
                key={ep.id}
                ref={(el) => {
                  if (el) rowRefs.current.set(ep.id, el);
                  else rowRefs.current.delete(ep.id);
                }}
                className={rowClass}
              >
                <span className="ep-num">{ep.number}</span>
                <div className="ep-main">
                  <div className="ep-title" onClick={() => openEpisode(ep.url)}>
                    {t("common.episodeNumber", { number: ep.number })}
                  </div>
                  {ep.download_status === "failed" && (
                    <div className="dl-error">
                      {dl[ep.id]?.error || t("download.failed")}
                    </div>
                  )}
                </div>
                <div className="ep-actions">
                  {ep.download_status === "downloaded" ? (
                    <>
                      <button
                        type="button"
                        className="btn btn-sm btn-primary"
                        title={t("download.downloaded")}
                        onClick={() => playDownload(ep.id).catch(console.error)}
                      >
                        {t("download.playOffline")}
                      </button>
                      <button
                        type="button"
                        className="btn btn-sm btn-ghost"
                        title={t("download.deleteKeepsWatched")}
                        aria-label={t("download.delete")}
                        onClick={() => deleteDownload(ep.id).then(() => load()).catch(console.error)}
                      >
                        🗑
                      </button>
                    </>
                  ) : ep.download_status === "downloading" ? (
                    <>
                      <span className="dl-badge dl-active">
                        {typeof dl[ep.id]?.percent === "number"
                          ? t("download.downloading", { percent: dl[ep.id]?.percent! })
                          : t("download.queued")}
                      </span>
                      <button
                        type="button"
                        className="btn btn-sm"
                        title={t("download.cancel")}
                        onClick={() => cancelDownload(ep.id).catch(console.error)}
                      >
                        ✕
                      </button>
                    </>
                  ) : ep.download_status === "failed" ? (
                    <button
                      type="button"
                      className="btn btn-sm"
                      title={`${t("download.failed")}${dl[ep.id]?.error ? `: ${dl[ep.id]?.error}` : ""}`}
                      onClick={() => {
                        setDl((prev) => ({ ...prev, [ep.id]: { percent: 0, error: undefined } }));
                        downloadEpisode(ep.id)
                          .then(() => load())
                          .catch(console.error);
                      }}
                    >
                      ⚠ {t("download.retry")}
                    </button>
                  ) : (
                    <button
                      type="button"
                      className="btn btn-sm"
                      title={t("download.download")}
                      onClick={() => {
                        setDl((prev) => ({ ...prev, [ep.id]: { percent: 0 } }));
                        downloadEpisode(ep.id)
                          .then(() => load())
                          .catch(console.error);
                      }}
                    >
                      ⬇
                    </button>
                  )}
                  <button type="button" className="btn" onClick={() => openEpisode(ep.url)}>
                    {t("common.watch")}
                  </button>
                  <button
                    type="button"
                    className={`check ${ep.seen ? "on" : ""}`}
                    title={ep.seen ? t("common.markUnseen") : t("common.markSeen")}
                    onClick={() => toggleSeen(ep)}
                  >
                    ✓
                  </button>
                  {ep.download_status === "downloading" && typeof dl[ep.id]?.percent === "number" && (
                    <div className="dl-progress">
                      <div
                        className="dl-progress-fill"
                        style={{ width: `${dl[ep.id]?.percent}%` }}
                      />
                    </div>
                  )}
                </div>
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
}
