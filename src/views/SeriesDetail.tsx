import { useEffect, useRef, useState } from "react";
import {
  downloadEpisode,
  getCatalogInfoForSeries,
  getNextAir,
  linkCatalogSeries,
  listEpisodes,
  openEpisode,
  reclassifySeries,
  setSeenCascade,
  getPlayerSettings,
  setPlayerSettings,
} from "../api";

import { useT } from "../i18n";
import { airCountdown } from "../lib/airCountdown";
import { isUnlinkedCatalogRow } from "../lib/catalogLink";
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
            // isn't linked to the catalog.
            href={catalogInfo?.url ?? series.url}
            target="_blank"
            rel="noreferrer"
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

      {linking ? (
        <div className="empty">{t("seriesDetail.linking")}</div>
      ) : loading ? (
        <div className="empty">{t("seriesDetail.loadingEpisodes")}</div>
      ) : episodes.length === 0 ? (
        <div className="empty">{t("seriesDetail.noEpisodes")}</div>
      ) : (
        <div className="series-block">
          {episodes.map((ep) => (
            <div
              key={ep.id}
              ref={(el) => {
                if (el) rowRefs.current.set(ep.id, el);
                else rowRefs.current.delete(ep.id);
              }}
              className={`ep-row ${ep.seen ? "seen" : ""}`}
            >
              <span className="ep-num">{ep.number}</span>
              <div className="ep-main">
                <div className="ep-title" onClick={() => openEpisode(ep.url)}>
                  {t("common.episodeNumber", { number: ep.number })}
                </div>
              </div>
              <div className="ep-actions">
                {ep.download_status === "downloaded" ? (
                  <span className="dl-badge" title={t("download.downloaded")}>💾</span>
                ) : ep.download_status === "downloading" ? (
                  <span className="dl-badge dl-active" title={t("download.downloading", { percent: "…" })}>⏳</span>
                ) : (
                  <button
                    className="btn btn-sm"
                    title={t("download.download")}
                    onClick={() => downloadEpisode(ep.id).catch(console.error)}
                  >
                    ⬇
                  </button>
                )}
                <button className="btn" onClick={() => openEpisode(ep.url)}>
                  {t("common.watch")}
                </button>
                <button
                  className={`check ${ep.seen ? "on" : ""}`}
                  title={ep.seen ? t("common.markUnseen") : t("common.markSeen")}
                  onClick={() => toggleSeen(ep)}
                >
                  ✓
                </button>
              </div>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
