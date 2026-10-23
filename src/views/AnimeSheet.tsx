import { useCallback, useEffect, useMemo, useRef, useState, type KeyboardEvent, type ReactNode } from "react";
import { createPortal } from "react-dom";
import { getAnimeExtras, openEpisode } from "../api";
import { useT, type MessageKey } from "../i18n";
import { airCountdown } from "../lib/airCountdown";
import { seasonOf } from "../lib/anilistLinks";
import { formatLabel } from "../lib/formatLabel";
import { useFormatNumber } from "../lib/formatNumber";
import { initials, resolveCoverUrl } from "../lib/posterFallback";
import type { AnimeExtras, CatalogAnime, Trailer } from "../types";
import { RelatedRow, SeriesAbout } from "./SeriesInfo";
import "../styles/animesheet.css";

// In-app info sheet for a catalog title: the data the catalog already holds
// plus AniList's description, relations and trailer (get_anime_extras). Used
// instead of sending the user to the AniList web page.

/** The minimum a sheet needs; everything else is shown when the caller has it. */
export type SheetAnime = Pick<CatalogAnime, "id" | "title" | "cover_url" | "url"> &
  Partial<Omit<CatalogAnime, "id" | "title" | "cover_url" | "url">>;

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

const FOCUSABLE =
  'a[href], button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), iframe, [tabindex]:not([tabindex="-1"])';

// `site` and `id` are validated by the backend ([A-Za-z0-9_-], <= 64 chars)
// and the CSP frame-src only allows these two hosts.
function trailerEmbedUrl(tr: Trailer): string {
  return tr.site === "dailymotion"
    ? `https://www.dailymotion.com/embed/video/${tr.id}?autoplay=1`
    : `https://www.youtube-nocookie.com/embed/${tr.id}?autoplay=1&rel=0`;
}

function trailerWatchUrl(tr: Trailer): string {
  return tr.site === "dailymotion"
    ? `https://www.dailymotion.com/video/${tr.id}`
    : `https://www.youtube.com/watch?v=${tr.id}`;
}

function TrailerBox({ trailer, title }: { trailer: Trailer; title: string }) {
  const t = useT();
  const [playing, setPlaying] = useState(false);
  const [thumbFailed, setThumbFailed] = useState(false);
  const youtube = trailer.site === "youtube";
  const watchUrl = trailerWatchUrl(trailer);

  return (
    <section className="sheet-trailer" aria-labelledby="sheet-trailer-title">
      <div className="sheet-trailer-head">
        <h3 id="sheet-trailer-title" className="section-title">
          {t("animeSheet.trailer")}
        </h3>
        <button
          type="button"
          className="btn-text"
          onClick={() => openEpisode(watchUrl).catch((err) => console.error("openEpisode failed for", watchUrl, err))}
        >
          {t(youtube ? "animeSheet.watchYoutube" : "animeSheet.watchDailymotion")}
        </button>
      </div>
      <div className="sheet-trailer-frame">
        {playing ? (
          <iframe
            src={trailerEmbedUrl(trailer)}
            title={t("animeSheet.trailerFrame", { title })}
            allow="autoplay; encrypted-media; picture-in-picture; fullscreen"
            allowFullScreen
            referrerPolicy="strict-origin-when-cross-origin"
          />
        ) : (
          <button type="button" className="sheet-trailer-play" onClick={() => setPlaying(true)}>
            {youtube && !thumbFailed && (
              <img
                src={`https://i.ytimg.com/vi/${trailer.id}/hqdefault.jpg`}
                alt=""
                loading="lazy"
                onError={() => setThumbFailed(true)}
              />
            )}
            <span className="sheet-trailer-icon" aria-hidden="true">
              ▶
            </span>
            <span className="sr-only">{t("animeSheet.playTrailer")}</span>
          </button>
        )}
      </div>
    </section>
  );
}

export interface AnimeSheetActions {
  /** "Empezar a ver": the caller runs its start-watching flow for this title. */
  onStart?: (anime: SheetAnime) => void | Promise<void>;
  onWant?: (anime: SheetAnime) => void | Promise<void>;
  onSeen?: (anime: SheetAnime) => void | Promise<void>;
}

function AnimeSheet({ anime, onClose, onStart, onWant, onSeen }: AnimeSheetActions & { anime: SheetAnime; onClose: () => void }) {
  const t = useT();
  const formatNumber = useFormatNumber();
  const dialogRef = useRef<HTMLDivElement>(null);
  const [extras, setExtras] = useState<AnimeExtras | null>(null);
  const [bannerFailed, setBannerFailed] = useState(false);
  const [coverFailed, setCoverFailed] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  // Load the description/relations/trailer; a failure just leaves them empty.
  useEffect(() => {
    let cancelled = false;
    setExtras(null);
    getAnimeExtras(anime.id)
      .then((e) => {
        if (!cancelled) setExtras(e);
      })
      .catch((err) => {
        console.error("getAnimeExtras failed for", anime.id, err);
        if (!cancelled) setExtras({ description: null, relations: [], trailer: null });
      });
    return () => {
      cancelled = true;
    };
  }, [anime.id]);

  // Clicking plain text inside the sheet drops focus to <body>, where the
  // dialog's own onKeyDown no longer sees keys: catch them on the way down so
  // Escape still closes and the host's window shortcuts (the deck's arrows)
  // never act behind the sheet.
  useEffect(() => {
    function onWindowKey(e: globalThis.KeyboardEvent) {
      if (dialogRef.current?.contains(e.target as Node)) return;
      e.stopPropagation();
      if (e.key === "Escape") {
        e.preventDefault();
        onClose();
      }
    }
    window.addEventListener("keydown", onWindowKey, true);
    return () => window.removeEventListener("keydown", onWindowKey, true);
  }, [onClose]);

  // Focus moves into the dialog and goes back to the opener on close.
  useEffect(() => {
    const opener = document.activeElement as HTMLElement | null;
    dialogRef.current?.querySelector<HTMLElement>(".sheet-close")?.focus();
    return () => opener?.focus?.();
  }, []);

  function onKeyDown(e: KeyboardEvent<HTMLDivElement>) {
    // Keep the host view's window-level shortcuts (the swipe deck's arrows)
    // from firing while the sheet is open.
    e.stopPropagation();
    if (e.key === "Escape") {
      e.preventDefault();
      onClose();
      return;
    }
    if (e.key !== "Tab") return;
    const items = [...(dialogRef.current?.querySelectorAll<HTMLElement>(FOCUSABLE) ?? [])];
    if (items.length === 0) return;
    const first = items[0];
    const last = items[items.length - 1];
    const active = document.activeElement;
    if (e.shiftKey && (active === first || !dialogRef.current?.contains(active))) {
      e.preventDefault();
      last.focus();
    } else if (!e.shiftKey && (active === last || !dialogRef.current?.contains(active))) {
      e.preventDefault();
      first.focus();
    }
  }

  async function run(action: ((a: SheetAnime) => void | Promise<void>) | undefined) {
    if (!action || busy) return;
    setBusy(true);
    try {
      await action(anime);
    } catch (err) {
      console.error("sheet action failed", err);
    } finally {
      setBusy(false);
    }
    onClose();
  }

  const banner = anime.banner_url && !bannerFailed ? anime.banner_url : null;
  const cover = resolveCoverUrl(anime.cover_url);
  const alts = [anime.title_romaji, anime.title_english]
    .filter((x): x is string => !!x && x.trim() !== "" && x.trim().toLowerCase() !== anime.title.trim().toLowerCase())
    .filter((x, i, all) => all.findIndex((y) => y.toLowerCase() === x.toLowerCase()) === i);

  const facts = useMemo(() => {
    const out: { label: string; value: string }[] = [];
    const statusKey = anime.status ? STATUS_KEYS[anime.status] : undefined;
    if (statusKey) out.push({ label: t("seriesDetail.info.status"), value: t(statusKey) });
    if (anime.format) out.push({ label: t("seriesDetail.info.format"), value: formatLabel(t, anime.format) });
    if (anime.episodes != null) out.push({ label: t("seriesDetail.info.episodes"), value: String(anime.episodes) });
    if (anime.duration != null && anime.duration > 0) {
      out.push({
        label: t("seriesDetail.info.duration"),
        value: t("seriesDetail.info.durationValue", { n: anime.duration }),
      });
    }
    const season = seasonOf(anime.start_date);
    if (season) {
      out.push({ label: t("seriesDetail.info.season"), value: t(SEASON_KEYS[season.season], { year: season.year }) });
    }
    if (anime.studio) out.push({ label: t("seriesDetail.info.studio"), value: anime.studio });
    if (anime.average_score != null) out.push({ label: t("seriesDetail.info.score"), value: `${anime.average_score}/100` });
    if (anime.popularity != null) out.push({ label: t("animeSheet.popularity"), value: formatNumber(anime.popularity) });
    if (anime.next_airing_at != null) {
      const c = airCountdown(anime.next_airing_at, Date.now() / 1000);
      if (c) {
        const when = t(
          c.unit === "minutes"
            ? "seriesDetail.nextAir.minutes"
            : c.unit === "hours"
            ? "seriesDetail.nextAir.hours"
            : "seriesDetail.nextAir.days",
          { n: c.value }
        );
        out.push({
          label: t("animeSheet.nextEpisode"),
          value: anime.next_episode != null ? `Ep ${anime.next_episode} · ${when}` : when,
        });
      }
    }
    return out;
  }, [anime, t, formatNumber]);

  const genres = anime.genres ?? [];
  const hasActions = onStart || onWant || onSeen;

  return (
    <div
      className="sheet-backdrop"
      onMouseDown={(e) => {
        if (e.target === e.currentTarget) onClose();
      }}
    >
      <div
        ref={dialogRef}
        className="sheet-dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby="sheet-title"
        onKeyDown={onKeyDown}
      >
        <button type="button" className="sheet-close" onClick={onClose} aria-label={t("animeSheet.close")}>
          ✕
        </button>
        <div className="sheet-scroll">
          {banner ? (
            <div className="sheet-banner">
              <img src={banner} alt="" onError={() => setBannerFailed(true)} />
            </div>
          ) : (
            <div className="sheet-banner sheet-banner-empty" />
          )}

          <div className="sheet-head">
            <div className="sheet-cover">
              {cover && coverFailed !== cover ? (
                <img src={cover} alt="" onError={() => setCoverFailed(cover)} />
              ) : (
                <span className="sheet-cover-fallback" aria-hidden="true">
                  {initials(anime.title)}
                </span>
              )}
            </div>
            <div className="sheet-head-text">
              <h2 id="sheet-title" className="sheet-title">
                {anime.title}
              </h2>
              {alts.map((alt) => (
                <div key={alt} className="muted sheet-alt">
                  {alt}
                </div>
              ))}
            </div>
          </div>

          <div className="sheet-body">
            {facts.length > 0 && (
              <dl className="sheet-facts">
                {facts.map((f) => (
                  <div key={f.label} className="sheet-fact">
                    <dt>{f.label}</dt>
                    <dd>{f.value}</dd>
                  </div>
                ))}
              </dl>
            )}

            {genres.length > 0 && (
              <ul className="sheet-genres" aria-label={t("animeSheet.genres")}>
                {genres.map((g) => (
                  <li key={g} className="tag">
                    {g}
                  </li>
                ))}
              </ul>
            )}

            <SeriesAbout
              key={anime.id}
              siteSynopsis={null}
              siteName={null}
              anilistDescription={extras?.description ?? null}
              anilistLoading={extras === null}
              tags={anime.tags ?? []}
            />

            {extras?.trailer && <TrailerBox trailer={extras.trailer} title={anime.title} />}

            <RelatedRow items={extras?.relations ?? []} />
          </div>
        </div>

        <div className="sheet-actions">
          {onStart && (
            <button type="button" className="btn btn-primary" disabled={busy} onClick={() => run(onStart)}>
              {t("catalog.batchStart")}
            </button>
          )}
          {onWant && (
            <button type="button" className="btn" disabled={busy} onClick={() => run(onWant)}>
              {t("catalog.batchWant")}
            </button>
          )}
          {onSeen && (
            <button type="button" className="btn" disabled={busy} onClick={() => run(onSeen)}>
              {t("catalog.batchSeen")}
            </button>
          )}
          <button
            type="button"
            className={hasActions ? "btn btn-ghost" : "btn"}
            onClick={() => openEpisode(anime.url).catch((err) => console.error("openEpisode failed for", anime.url, err))}
          >
            {t("seriesDetail.openPage")}
          </button>
        </div>
      </div>
    </div>
  );
}

/** `const { open, element } = useAnimeSheet({ onWant, ... })`, then render `{element}`. */
export function useAnimeSheet(actions: AnimeSheetActions = {}): {
  open: (anime: SheetAnime) => void;
  element: ReactNode;
} {
  const [anime, setAnime] = useState<SheetAnime | null>(null);
  const close = useCallback(() => setAnime(null), []);
  const element = anime
    ? createPortal(<AnimeSheet anime={anime} onClose={close} {...actions} />, document.body)
    : null;
  return { open: setAnime, element };
}
