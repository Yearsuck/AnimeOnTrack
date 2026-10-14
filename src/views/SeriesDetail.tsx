import { listen } from "@tauri-apps/api/event";
import { useEffect, useMemo, useRef, useState } from "react";
import {
  cancelDownload,
  deleteDownload,
  deleteSeriesDownloads,
  downloadEpisode,
  downloadPending,
  getCatalogInfoForSeries,
  getNextAir,
  getPlayerSettings,
  linkCatalogSeries,
  listEpisodes,
  openEpisode,
  playDownload,
  reclassifySeries,
  setPlayerSettings,
  setSeenCascade,
} from "../api";

import { useT } from "../i18n";
import { airCountdown } from "../lib/airCountdown";
import { isUnlinkedCatalogRow } from "../lib/catalogLink";
import { summarizeDownloads } from "../lib/downloadSummary";
import { epNum } from "../lib/episodeNumber";
import { resolveCoverUrl } from "../lib/posterFallback";
import type { CatalogAnime, Episode, NextAir, Series } from "../types";

export function SeriesDetail({
  series,
  onBack,
  onChanged,
}: {
  series: Series;
  onBack: () => void;
  onChanged: () => void;
}) {
  const t = useT();
  const [episodes, setEpisodes] = useState<Episode[]>([]);
  const [loading, setLoading] = useState(true);
  const [linking, setLinking] = useState(false);
  // Anime metadata (genres/studio/format/episodes/score/cover/AniList url)
  // comes from AniList, not the scraped site — the site only still supplies
  // episode links and the "is it airing" signal (see `listEpisodes` below).
  const [catalogInfo, setCatalogInfo] = useState<CatalogAnime | null>(null);
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
  useEffect(() => {
    setCatalogInfo(null);
    getCatalogInfoForSeries(series.id)
      .then(setCatalogInfo)
      .catch(() => setCatalogInfo(null));
  }, [series.id]);

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

  useEffect(() => {
    const unlisten = listen<{
      episode_id: number;
      status: "downloading" | "downloaded" | "failed" | "cancelled";
      percent: number | null;
      error: string | null;
    }>("download-progress", (e) => {
      const p = e.payload;
      setDl((prev) => ({
        ...prev,
        [p.episode_id]: { percent: p.percent, error: p.error ?? undefined },
      }));
      if (p.status !== "downloading") {
        load().catch(() => {});
      }
    });
    return () => {
      unlisten.then((f) => f()).catch(() => {});
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
    setSeenCascade(series.id, ep.number, nextSeen).then(onChanged);
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
    <div className="page">
      <button className="btn btn-ghost detail-back" onClick={onBack}>
        {t("common.back")}
      </button>

      <div className="page-head detail-head">
        {detailCover && (
          <img className="detail-cover" src={detailCover} alt="" />
        )}
        <div className="detail-main">
          <h2 className="page-title detail-title">{catalogInfo?.title ?? series.title}</h2>
          <a
            // The "open page" link points at AniList — the reliable source of
            // truth for anime info. The pirate site only supplies episodes (the
            // play buttons below), so it's the fallback only when this show
            // isn't linked to the catalog. Opens inside an app-owned WebView2
            // window instead of spawning the system browser.
            href={catalogInfo?.url ?? series.url}
            onClick={(e) => {
              e.preventDefault();
              const url = catalogInfo?.url ?? series.url;
              openEpisode(url).catch((err) =>
                console.error("openEpisode failed for", url, err)
              );
            }}
          >
            {t("seriesDetail.openPage")}
          </a>
          {catalogInfo ? (
            <div className="muted detail-meta">
              {catalogInfo.studio && (
                <span>{t("seriesDetail.info.studio")}: {catalogInfo.studio}</span>
              )}
              {catalogInfo.format && (
                <span>{t("seriesDetail.info.format")}: {catalogInfo.format}</span>
              )}
              {catalogInfo.episodes != null && (
                <span>{t("seriesDetail.info.episodes")}: {catalogInfo.episodes}</span>
              )}
              {catalogInfo.average_score != null && (
                <span>{t("seriesDetail.info.score")}: {catalogInfo.average_score}/100</span>
              )}
            </div>
          ) : (
            <div className="muted detail-meta">{t("seriesDetail.info.notSynced")}</div>
          )}
          {nextAir && countdown && (
            <p className="muted detail-next-air">
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
        </div>
        {episodes.length > 0 && (
          <button
            className={`btn ${playerFullscreen ? "btn-primary" : "btn-ghost"}`}
            onClick={togglePlayerFullscreen}
            title={t("settings.playerHelp")}
          >
            {playerFullscreen ? `⛶ ${t("settings.playerFullscreen")}` : `🗗 ${t("settings.playerWindowed")}`}
          </button>
        )}
        {episodes.length > 0 && (
          <button className="btn" onClick={jumpToCurrent}>
            {t("seriesDetail.jumpToCurrent")}
          </button>
        )}

        {series.followed && (
          <button className="btn btn-ghost" onClick={unfollow}>
            {t("seriesDetail.unfollow")}
          </button>
        )}
      </div>

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
