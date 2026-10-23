import { useEffect, useState } from "react";
import { getAnimeExtras } from "../../api";
import type { useT } from "../../i18n";
import { parseDescription, type DescParagraph } from "../../lib/anilistDescription";
import { formatLabel } from "../../lib/formatLabel";
import type { SwipeCard } from "../../types";

export interface DeckCardInfo {
  /** The other title AniList knows it by, when it differs from the shown one. */
  alt: string | null;
  /** "TV · 2023 · 28 episodios · Madhouse" */
  facts: string;
  genres: string[];
  /** "91%", or null when AniList has no score. */
  score: string | null;
}

/** What the deck shows about a card, from the catalog row it was picked from. */
export function deckCardInfo(card: SwipeCard, t: ReturnType<typeof useT>): DeckCardInfo {
  const c = card.catalog;
  const year = c?.start_date != null ? new Date(c.start_date * 1000).getUTCFullYear() : null;
  const upcoming = c?.status === "NOT_YET_RELEASED";
  const format = c?.format ?? card.kind;
  const facts = [
    format ? formatLabel(t, format) : null,
    upcoming ? t("filters.statusNotYetReleased") : year != null ? String(year) : null,
    c?.episodes ? (c.episodes === 1 ? t("catalog.episodesOne") : t("catalog.episodesCount", { count: c.episodes })) : null,
    c?.studio ?? null,
  ]
    .filter(Boolean)
    .join(" · ");
  const alt = [c?.title_romaji, c?.title_english].find(
    (x): x is string => !!x && x.trim() !== "" && x.trim().toLowerCase() !== card.title.trim().toLowerCase()
  );
  return {
    alt: alt ?? null,
    facts,
    genres: c?.genres?.length ? c.genres : card.matched_genre ? [card.matched_genre] : [],
    score: c?.average_score ? `${c.average_score}%` : null,
  };
}

/** A description as plain text with spoiler passages left out (the deck shows a teaser, not the sheet). */
export function teaserText(paras: DescParagraph[]): string {
  return paras
    .map((p) => p.filter((s) => !s.spoiler).map((s) => s.text).join(" ").trim())
    .filter(Boolean)
    .join(" ");
}

// Synopses already fetched this session, by AniList id ("" = AniList has none).
const synopsisCache = new Map<number, string>();
// Only a card the user actually looks at costs an AniList request: swiping
// through quickly never fetches.
const DWELL_MS = 900;

/**
 * The current card's synopsis teaser: `null` while it is being fetched, `""`
 * when there is none (or the fetch failed).
 */
export function useCardSynopsis(card: SwipeCard | null): string | null {
  const id = card?.catalog?.id ?? null;
  // Tagged with the id it belongs to, so the render where the card changes
  // never shows the previous card's synopsis under the new title.
  const [fetched, setFetched] = useState<{ id: number; text: string } | null>(null);

  useEffect(() => {
    if (id == null || synopsisCache.has(id)) return;
    let alive = true;
    const timer = setTimeout(() => {
      getAnimeExtras(id)
        .then((e) => {
          const text = teaserText(parseDescription(e.description));
          // Only a real answer is cached: a failed fetch (network, rate
          // limit) is retried the next time the card comes up.
          synopsisCache.set(id, text);
          if (alive) setFetched({ id, text });
        })
        .catch((err) => {
          console.error("getAnimeExtras failed for", id, err);
          if (alive) setFetched({ id, text: "" });
        });
    }, DWELL_MS);
    return () => {
      alive = false;
      clearTimeout(timer);
    };
  }, [id]);

  if (id == null) return "";
  const cached = synopsisCache.get(id);
  if (cached !== undefined) return cached;
  return fetched?.id === id ? fetched.text : null;
}
