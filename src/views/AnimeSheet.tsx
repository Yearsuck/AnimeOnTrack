import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, type KeyboardEvent, type ReactNode } from "react";
import { useBackLayer } from "../lib/backStack";
import { createPortal } from "react-dom";
import { getAnimeExtras, openEpisode } from "../api";
import { useT, type MessageKey } from "../i18n";
import { countdown } from "../lib/airCountdown";
import { seasonOf } from "../lib/anilistLinks";
import { formatLabel } from "../lib/formatLabel";
import { FALLBACK_TRANSFORM, flipDelta, flipTransform, type Box } from "../lib/flip";
import { useFormatNumber } from "../lib/formatNumber";
import { initials, resolveCoverUrl } from "../lib/posterFallback";
import { isMobile } from "../lib/platform";
import type { AnimeExtras, CatalogAnime } from "../types";
import { TrailerBox } from "../components/TrailerBox";
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

export interface AnimeSheetActions {
  /** "Empezar a ver": the caller runs its start-watching flow for this title. */
  onStart?: (anime: SheetAnime) => void | Promise<void>;
  onWant?: (anime: SheetAnime) => void | Promise<void>;
  onSeen?: (anime: SheetAnime) => void | Promise<void>;
}

const ENTER_MS = 320;
const EXIT_MS = 220;
const EASE_OUT = "cubic-bezier(0.16, 1, 0.3, 1)";

function prefersReducedMotion(): boolean {
  try {
    return window.matchMedia("(prefers-reduced-motion: reduce)").matches;
  } catch {
    return false;
  }
}

/** The box a sheet should grow from: the clicked poster/cover, if there is one. */
function originBox(el: Element | null | undefined): Box | null {
  if (!el) return null;
  const card = el.closest(".poster-card");
  const hero = el.closest(".cat-hero");
  const target = card?.querySelector(".poster") ?? hero?.querySelector(".cat-hero-cover") ?? (card || hero ? null : el);
  if (!target) return null;
  const r = target.getBoundingClientRect();
  return r.width > 0 && r.height > 0 ? { left: r.left, top: r.top, width: r.width, height: r.height } : null;
}

function AnimeSheet({
  anime,
  origin,
  originEl,
  onClose: onClosed,
  onStart,
  onWant,
  onSeen,
}: AnimeSheetActions & { anime: SheetAnime; origin: Box | null; originEl: Element | null; onClose: () => void }) {
  const t = useT();
  const backdropRef = useRef<HTMLDivElement>(null);
  const closingRef = useRef(false);
  const unmount = onClosed;
  const closedRef = useRef(FALLBACK_TRANSFORM);
  const formatNumber = useFormatNumber();
  const dialogRef = useRef<HTMLDivElement>(null);
  const [extras, setExtras] = useState<AnimeExtras | null>(null);
  const [bannerFailed, setBannerFailed] = useState(false);
  const [coverFailed, setCoverFailed] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  // Closed-state keyframe: the origin's footprint (centre-anchored scale) or a
  // small settle when the origin is unknown. Reduced motion: opacity only.
  const finalRectRef = useRef<DOMRect | null>(null);
  const closedTransform = useCallback(
    (from: Box | null): string => {
      if (prefersReducedMotion() || isMobile) return "none";
      const rect = finalRectRef.current;
      const d = from && rect ? flipDelta(from, rect) : null;
      return d ? flipTransform(d) : FALLBACK_TRANSFORM;
    },
    []
  );

  // Grow in: runs before first paint, so the dialog never flashes at full size.
  // Only transform + opacity animate (compositor). StrictMode runs this twice
  // in dev: running animations are cancelled before measuring so the second
  // run reads the final box, and the cleanup cancels what this run started.
  useLayoutEffect(() => {
    const dlg = dialogRef.current;
    const back = backdropRef.current;
    if (!dlg || !back || typeof dlg.animate !== "function") return;
    dlg.getAnimations().forEach((a) => a.cancel());
    back.getAnimations().forEach((a) => a.cancel());
    finalRectRef.current = dlg.getBoundingClientRect();
    const reduce = prefersReducedMotion();
    const from = closedTransform(origin);
    closedRef.current = from;
    const anims = [
      back.animate([{ opacity: 0 }, { opacity: 1 }], { duration: reduce ? 140 : 200, easing: "ease-out" }),
      dlg.animate(
        [
          { transform: from, opacity: 0 },
          { transform: from, opacity: 1, offset: 0.18 },
          { transform: "none", opacity: 1 },
        ],
        { duration: reduce ? 140 : ENTER_MS, easing: EASE_OUT }
      ),
    ];
    return () => {
      if (!closingRef.current) anims.forEach((a) => a.cancel());
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // Shrink back toward the origin, then unmount. Starts from wherever the
  // enter animation currently is, so closing mid-open does not jump.
  const onClose = useCallback(() => {
    if (closingRef.current) return;
    closingRef.current = true;
    const dlg = dialogRef.current;
    const back = backdropRef.current;
    if (!dlg || !back || typeof dlg.animate !== "function") {
      unmount();
      return;
    }
    const cs = getComputedStyle(dlg);
    const fromT = cs.transform;
    const fromO = cs.opacity;
    const bo = getComputedStyle(back).opacity;
    dlg.getAnimations().forEach((a) => a.cancel());
    back.getAnimations().forEach((a) => a.cancel());
    const reduce = prefersReducedMotion();
    // Re-measure the origin if it is still on screen (the list may have scrolled).
    const live = originEl?.isConnected ? originBox(originEl) : null;
    const to = live ? closedTransform(live) : closedRef.current;
    back.animate([{ opacity: bo }, { opacity: 0 }], { duration: EXIT_MS, easing: "ease-out", fill: "forwards" });
    const exit = dlg.animate(
      [
        { transform: fromT, opacity: fromO },
        { transform: to, opacity: 0 },
      ],
      { duration: reduce ? 120 : EXIT_MS, easing: EASE_OUT, fill: "forwards" }
    );
    exit.onfinish = unmount;
    exit.oncancel = unmount;
  }, [unmount, originEl, closedTransform]);
  // System back closes the sheet like Escape does (animated).
  useBackLayer(true, onClose);

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
      const isMovie = anime.format?.toUpperCase() === "MOVIE";
      out.push({
        label: t("seriesDetail.info.duration"),
        value: isMovie
          ? t("seriesDetail.info.durationMovie", { n: anime.duration })
          : t("seriesDetail.info.durationValue", { n: anime.duration }),
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
      const when = countdown(anime.next_airing_at, Math.floor(Date.now() / 1000), { t });
      if (when) {
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
      ref={backdropRef}
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
  /** `origin`: the clicked element; the sheet grows out of its poster/cover. When omitted, a focused poster card is used, else a plain scale+fade. */
  open: (anime: SheetAnime, origin?: Element | null) => void;
  element: ReactNode;
} {
  const [shown, setShown] = useState<{ anime: SheetAnime; origin: Box | null; originEl: Element | null } | null>(null);
  const close = useCallback(() => setShown(null), []);
  const open = useCallback((anime: SheetAnime, origin?: Element | null) => {
    // Measured now, at click time, while the card is still where it was.
    const active = document.activeElement;
    const el = origin ?? (active?.closest(".poster-card") ? active : null);
    const box = originBox(el);
    setShown({ anime, origin: box, originEl: box ? el : null });
  }, []);
  const element = shown
    ? createPortal(<AnimeSheet anime={shown.anime} origin={shown.origin} originEl={shown.originEl} onClose={close} {...actions} />, document.body)
    : null;
  return { open, element };
}
