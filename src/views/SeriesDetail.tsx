import { useEffect, useRef, useState } from "react";
import {
  getCatalogInfoForSeries,
  linkCatalogSeries,
  listEpisodes,
  openEpisode,
  reclassifySeries,
  setSeenCascade,
} from "../api";
import { useT } from "../i18n";
import { isUnlinkedCatalogRow } from "../lib/catalogLink";
import type { CatalogAnime, Episode, Series } from "../types";

// Mirrors the backend's parse_ep_number (src-tauri/src/db.rs): leading
// digits (+ optional decimal) only, e.g. "12" -> 12, "12.5" -> 12.5,
// "1x05" -> 1. Numbers with no leading digit (e.g. "OVA") return null so
// the optimistic update falls back to an exact-string match instead of
// guessing an order — must stay in sync with the backend or the optimistic
// UI will disagree with what actually got persisted.
function epNum(number: string): number | null {
  const m = number.trim().match(/^\d+(\.\d+)?/);
  return m ? parseFloat(m[0]) : null;
}

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
  const rowRefs = useRef<Map<number, HTMLDivElement>>(new Map());
  // Guards the link-on-open trigger against React StrictMode's dev-only
  // double-invoke (same pattern as App.tsx's startup effect) — otherwise
  // opening an unlinked catalog row would fire two scrapes.
  const linkTriedRef = useRef<number | null>(null);

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

  return (
    <div className="page">
      <button className="btn btn-ghost" onClick={onBack} style={{ marginBottom: 14 }}>
        {t("common.back")}
      </button>

      <div className="page-head" style={{ alignItems: "flex-start" }}>
        {(catalogInfo?.cover_url ?? series.cover_url) && (
          <img
            src={catalogInfo?.cover_url ?? series.cover_url ?? undefined}
            alt=""
            style={{ width: 90, height: 128, objectFit: "cover", borderRadius: 10 }}
          />
        )}
        <div style={{ flex: 1 }}>
          <h2 className="page-title" style={{ marginBottom: 6 }}>
            {series.title}
          </h2>
          <a
            href={series.url}
            target="_blank"
            rel="noreferrer"
            onClick={() => {
              // Middle-click/ctrl-click already opens series.url itself via
              // the href; this only adds the AniList tab alongside it.
              if (catalogInfo?.url) window.open(catalogInfo.url, "_blank", "noreferrer");
            }}
          >
            {t("seriesDetail.openPage")}
          </a>
          {catalogInfo ? (
            <div className="muted" style={{ marginTop: 8, fontSize: 12.5, display: "flex", flexWrap: "wrap", gap: "4px 14px" }}>
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
            <div className="muted" style={{ marginTop: 8, fontSize: 12.5 }}>
              {t("seriesDetail.info.notSynced")}
            </div>
          )}
          {catalogInfo != null && catalogInfo.genres.length > 0 && (
            <div style={{ marginTop: 8, display: "flex", flexWrap: "wrap", gap: 6 }}>
              {catalogInfo.genres.map((g) => (
                <span key={g} className="chip-toggle" style={{ cursor: "default" }}>
                  {g}
                </span>
              ))}
            </div>
          )}
          <div style={{ marginTop: 10 }}>
            <div className="muted" style={{ marginBottom: 4, fontSize: 12.5 }}>
              {t("seriesDetail.seenCount", { seen: seenCount, total: episodes.length })}
            </div>
            <div className="progress">
              <span style={{ width: `${pct}%` }} />
            </div>
          </div>
        </div>
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
                  {ep.title ?? t("common.episodeNumber", { number: ep.number })}
                </div>
                {ep.released_at && <div className="ep-date">{ep.released_at}</div>}
              </div>
              <div className="ep-actions">
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
