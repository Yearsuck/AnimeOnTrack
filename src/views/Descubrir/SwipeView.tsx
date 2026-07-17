import { useCallback, useEffect, useRef, useState } from "react";
import {
  decideCatalogCard,
  discoverCatalogCard,
  listSwipeHistory,
  openEpisode,
  reclassifySeries,
  undoLastSwipe,
  undoSwipeEntry,
} from "../../api";
import { useDiscoverMode } from "../../discoverMode";
import { useT } from "../../i18n";
import type { SwipeCard, SwipeDecision, SwipeHistoryItem } from "../../types";
import {
  MAX_FILL_ROUNDS,
  PREFETCH_TARGET,
  RECLASSIFY_TARGET,
  REFILL_THRESHOLD,
} from "./constants";
import type { SwipeOutDirection } from "./types";
import { anilistIdFromUrl } from "./helpers";
import { DeckPanel } from "./DeckPanel";
import { HistoryRow } from "./HistoryRow";
import { TasteChips } from "./TasteChips";
import { useLinkQueue } from "./useLinkQueue";

export function SwipeView() {
  const t = useT();
  const [card, setCard] = useState<SwipeCard | null>(null);
  const [outDirection, setOutDirection] = useState<SwipeOutDirection>(null);
  const [canUndo, setCanUndo] = useState(false);
  const [exhausted, setExhausted] = useState(false);
  const [loading, setLoading] = useState(true);
  const busyRef = useRef(false);
  const queueRef = useRef<SwipeCard[]>([]);
  const fillingRef = useRef(false);
  const cardUrlRef = useRef<string | null>(null);
  // Every url decided this session (Discard/Want/Seen), so a concurrent
  // fillQueue round can never re-serve a card the user already swiped even
  // if its decideCatalogCard upsert hasn't landed in the DB yet (the
  // reappearance race: the deck only excludes a card once it's persisted).
  // Cleared per-url when the card legitimately returns to the deck (undo /
  // returnToDeck).
  const decidedUrlsRef = useRef<Set<string>>(new Set());
  // The single most-recently-decided url, so undo() (which only pops the
  // front of the backend's swipe_history — one step back) knows which url
  // to release from decidedUrlsRef.
  const lastDecidedUrlRef = useRef<string | null>(null);
  const { statuses: linkStatuses, enqueue: enqueueLink } = useLinkQueue();
  const [discoverMode] = useDiscoverMode();
  const isFirstModeRenderRef = useRef(true);
  // Multi-level undo cache: the last ~5 classified cards, newest first.
  const [history, setHistory] = useState<SwipeHistoryItem[]>([]);
  const refreshHistory = useCallback(() => {
    listSwipeHistory()
      .then(setHistory)
      .catch((err) => console.error("listSwipeHistory failed", err));
  }, []);

  // Top the local queue back up to PREFETCH_TARGET, deduping against
  // whatever's already queued or on screen (discover_catalog_card can hand
  // out the same not-yet-decided card twice — it only excludes cards
  // already persisted to the DB, and a prefetched card isn't persisted
  // until the user actually decides on it).
  const fillQueue = useCallback(async () => {
    if (fillingRef.current) return;
    fillingRef.current = true;
    try {
      for (let round = 0; round < MAX_FILL_ROUNDS && queueRef.current.length < PREFETCH_TARGET; round++) {
        const need = PREFETCH_TARGET - queueRef.current.length;
        const results = await Promise.all(
          Array.from({ length: need }, () => discoverCatalogCard(discoverMode === "recommended"))
        );
        const seen = new Set(queueRef.current.map((c) => c.url));
        if (cardUrlRef.current) seen.add(cardUrlRef.current);
        decidedUrlsRef.current.forEach((url) => seen.add(url));
        // Walk results in order, adding each accepted card's url to `seen`
        // as it's accepted — this also dedups two concurrent pickers in the
        // SAME round returning the same not-yet-persisted card (both would
        // pass a `seen` set built only from before the round started).
        const fresh: SwipeCard[] = [];
        for (const c of results) {
          if (c !== null && !seen.has(c.url)) {
            seen.add(c.url);
            fresh.push(c);
          }
        }
        // No forward progress this round (either truly empty, or every hit
        // was a duplicate of something already queued/decided) — stop rather
        // than burning more rounds of concurrent fetches chasing the same
        // handful of not-yet-decided cards.
        if (fresh.length === 0) break;
        queueRef.current = [...queueRef.current, ...fresh];
      }
    } finally {
      fillingRef.current = false;
    }
  }, [discoverMode]);

  // Pop the next card off the local queue (instant, no network wait in the
  // common case) and kick off a background refill once the buffer runs
  // low. If the queue is genuinely dry right now, fall back to an async
  // fill-then-retry instead of immediately declaring the deck exhausted —
  // that's only true once a real fill attempt comes back empty.
  const popNext = useCallback(() => {
    const next = queueRef.current[0];
    if (next) {
      queueRef.current = queueRef.current.slice(1);
      cardUrlRef.current = next.url;
      setCard(next);
      setExhausted(false);
      if (queueRef.current.length <= REFILL_THRESHOLD) {
        fillQueue();
      }
      return;
    }
    cardUrlRef.current = null;
    setCard(null);
    setLoading(true);
    fillQueue().then(() => {
      setLoading(false);
      if (queueRef.current.length > 0) {
        popNext();
      } else {
        setExhausted(true);
      }
    });
  }, [fillQueue]);

  useEffect(() => {
    (async () => {
      setLoading(true);
      await fillQueue();
      popNext();
      setLoading(false);
      refreshHistory();
    })();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // Invalidates the local prefetch buffer and refills it under the
  // currently-active bans/mode, without discarding whatever card is already
  // on screen and without touching decidedUrlsRef (already-decided cards
  // stay excluded regardless). Shared by the discoverMode-change effect
  // below and by DeckPanel's onBansSaved: saving a ban must have the exact
  // same effect as switching mode, because SwipeView no longer unmounts when
  // the deck panel's save button is pressed (it lives inside the Swipe
  // section now) — up to PREFETCH_TARGET stale-banned cards would otherwise
  // keep being served from the old queue.
  const resetQueue = useCallback(() => {
    queueRef.current = [];
    fillQueue();
  }, [fillQueue]);

  // Switching Recomendado <-> Aleatorio must refresh the *upcoming* deck
  // immediately (see resetQueue above). Skip the very first run (mount
  // already filled the queue in the effect above with the mode current at
  // that time).
  useEffect(() => {
    if (isFirstModeRenderRef.current) {
      isFirstModeRenderRef.current = false;
      return;
    }
    resetQueue();
  }, [discoverMode, resetQueue]);

  const decide = useCallback(
    async (decision: SwipeDecision, direction: Exclude<SwipeOutDirection, null>) => {
      if (!card || busyRef.current) return;
      busyRef.current = true;
      const activeCard = card;
      // Synchronously, before anything async: a concurrent fillQueue round
      // fired from popNext() below must never re-serve this card, even
      // though its decideCatalogCard upsert is still in flight.
      decidedUrlsRef.current.add(activeCard.url);
      lastDecidedUrlRef.current = activeCard.url;
      setOutDirection(direction);
      setCanUndo(false);
      const anilistId = anilistIdFromUrl(activeCard.url);
      const decidePromise =
        anilistId === null
          ? Promise.resolve(null)
          : decideCatalogCard({
              anilistId,
              title: activeCard.title,
              url: activeCard.url,
              posterUrl: activeCard.poster_url,
              genres: activeCard.matched_genre ? [activeCard.matched_genre] : [],
              format: activeCard.kind,
              decision,
            });
      setTimeout(() => {
        setOutDirection(null);
        popNext(); // instant — already prefetched, no round-trip to wait on
        busyRef.current = false;
        decidePromise.then((seriesId) => {
          setCanUndo(true);
          // Linking is a real scrape (seconds) — it must not block the swipe,
          // so it's fired here without awaiting it, queued through
          // useLinkQueue so rapid swiping serializes the scrapes instead of
          // firing them all in parallel.
          //
          // Only `Seen` triggers a scrape here. `Want` must NEVER scrape —
          // swiping right is fast, cheap, local, and stays that way; a scrape
          // per right-swipe would hammer the site behind Cloudflare for
          // titles the user may never actually watch. A `Want` row is only
          // ever linked later, explicitly: via "Empezar a ver" (start_watching)
          // or the manual "Buscar en la web" retry button in Listas.
          if (seriesId !== null && decision === "Seen") {
            enqueueLink(seriesId, activeCard.title);
          }
          refreshHistory();
        });
      }, 160);
    },
    [card, popNext, enqueueLink, refreshHistory]
  );

  const undo = useCallback(async () => {
    if (!canUndo) return;
    setCanUndo(false);
    await undoLastSwipe();
    // The card is back in the deck (its series row was hard-deleted) — let
    // fillQueue serve it again.
    if (lastDecidedUrlRef.current) {
      decidedUrlsRef.current.delete(lastDecidedUrlRef.current);
      lastDecidedUrlRef.current = null;
    }
    refreshHistory();
  }, [canUndo, refreshHistory]);

  // History-strip actions. Re-classifying a past card moves it between lists
  // (local, via the shared reclassify inverse); returning it to the deck
  // hard-deletes its row so the picker offers it again.
  const reclassifyHistory = useCallback(
    async (item: SwipeHistoryItem, action: "discard" | "want" | "seen") => {
      await reclassifySeries(item.series_id, RECLASSIFY_TARGET[action]);
      refreshHistory();
    },
    [refreshHistory]
  );
  const returnToDeck = useCallback(
    async (item: SwipeHistoryItem) => {
      await undoSwipeEntry(item.series_id);
      // Same as undo(): the row is hard-deleted, so fillQueue must be
      // allowed to serve this url again.
      decidedUrlsRef.current.delete(item.url);
      if (lastDecidedUrlRef.current === item.url) lastDecidedUrlRef.current = null;
      refreshHistory();
    },
    [refreshHistory]
  );

  useEffect(() => {
    function onKeyDown(e: KeyboardEvent) {
      if (e.ctrlKey && e.key.toLowerCase() === "z") {
        e.preventDefault();
        undo();
        return;
      }
      if (e.key === "ArrowLeft") {
        e.preventDefault();
        decide("Discard", "discard");
      } else if (e.key === "ArrowRight") {
        e.preventDefault();
        decide("Want", "want");
      } else if (e.key === "ArrowUp") {
        e.preventDefault();
        decide("Seen", "seen");
      }
    }
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [decide, undo]);

  return (
    <div className="swipe-layout">
      <DeckPanel onBansSaved={resetQueue} />
      <div className="swipe-stage">
        <TasteChips />
        {exhausted ? (
          <div className="empty">{t("discover.exhausted")}</div>
        ) : card ? (
          <div className={`card swipe-card ${outDirection ? `swipe-out-${outDirection}` : ""}`}>
            <div
              className="poster"
              style={{ cursor: "pointer" }}
              title={t("discover.openPageTitle")}
              onClick={(e) => {
                e.stopPropagation();
                openEpisode(card.url).catch((err) =>
                  console.error("openEpisode failed for", card.url, err)
                );
              }}
            >
              <span className="chip">{card.kind}</span>
              {card.poster_url ? <img src={card.poster_url} alt={card.title} /> : null}
            </div>
            <div className="card-body">
              <div className="card-title" style={{ minHeight: "auto" }}>
                {card.title}
              </div>
              {card.matched_genre && (
                <div className="muted" style={{ fontSize: 12 }}>
                  {t("discover.genreLabel", { genre: card.matched_genre })}
                </div>
              )}
            </div>
          </div>
        ) : (
          <div className="card swipe-card">
            <div className="poster" />
            <div className="card-body">
              <div className="muted">{loading ? t("common.loading") : ""}</div>
            </div>
          </div>
        )}

        <div className="swipe-actions">
          <button
            className="btn btn-discard"
            title={t("discover.discardTitle")}
            onClick={() => decide("Discard", "discard")}
            disabled={!card}
          >
            ✕
          </button>
          <button
            className="btn btn-want"
            title={t("discover.wantTitle")}
            onClick={() => decide("Want", "want")}
            disabled={!card}
          >
            ★
          </button>
          <button
            className="btn btn-success"
            title={t("discover.seenTitle")}
            onClick={() => decide("Seen", "seen")}
            disabled={!card}
          >
            ✓
          </button>
          <button
            className="btn btn-ghost"
            title={t("discover.undoTitle")}
            onClick={undo}
            disabled={!canUndo}
          >
            ↺
          </button>
        </div>
        <div className="swipe-hint">{t("discover.hint")}</div>
        {linkStatuses.length > 0 && (
          <div style={{ display: "flex", flexDirection: "column", gap: 4, marginTop: 8 }}>
            {linkStatuses.map((s) => (
              <div key={s.id} className="muted" style={{ fontSize: 12 }}>
                {s.state === "searching" && t("discover.searching", { title: s.title })}
                {s.state === "linked" && t("discover.linked", { count: s.episodes ?? 0 })}
                {s.state === "nomatch" && t("discover.noMatch", { title: s.title })}
              </div>
            ))}
          </div>
        )}

        {history.length > 0 && (
          <div className="swipe-history">
            <div className="muted" style={{ fontSize: 12, marginBottom: 6 }}>
              {t("discover.historyHeading")}
            </div>
            {history.map((h) => (
              <HistoryRow
                key={h.series_id}
                item={h}
                onReclassify={reclassifyHistory}
                onReturn={returnToDeck}
              />
            ))}
          </div>
        )}
      </div>
    </div>
  );
}
