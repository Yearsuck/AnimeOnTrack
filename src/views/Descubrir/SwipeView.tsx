import { useCallback, useEffect, useRef, useState } from "react";
import {
  decideCatalogCard,
  discoverCatalogCard,
  discoverCatalogCards,
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
import { deckCardInfo, useCardSynopsis } from "./deckCard";
import "../../styles/discover.css";
import { useLinkQueue } from "./useLinkQueue";
import { resolveCoverUrl } from "../../lib/posterFallback";
import { formatLabel } from "../../lib/formatLabel";
import { useAnimeSheet } from "../AnimeSheet";


export function SwipeView() {
  const t = useT();
  const [card, setCard] = useState<SwipeCard | null>(null);
  const [outDirection, setOutDirection] = useState<SwipeOutDirection>(null);
  const [canUndo, setCanUndo] = useState(false);
  const [exhausted, setExhausted] = useState(false);
  const [loading, setLoading] = useState(true);
  const busyRef = useRef(false);
  const queueRef = useRef<SwipeCard[]>([]);
  const fillPromiseRef = useRef<Promise<void> | null>(null);
  // Bumped by resetQueue so an in-flight fillQueue round started under the
  // OLD filters can tell its results are stale once it resolves (see
  // fillQueue/resetQueue below) — a save-then-refetch race that otherwise
  // let a stale round silently refill the just-cleared queue with
  // old-filter cards, which then kept getting served until the buffer ran
  // out or the view remounted.
  const fillGenerationRef = useRef(0);
  // Set when resetQueue fires: tells the in-flight fillQueue run (if any) to go
  // round again under the new generation before it resolves.
  const pendingResetRef = useRef(false);
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
  const [discoverMode, setDiscoverMode] = useDiscoverMode();
  const discoverModeRef = useRef(discoverMode);
  discoverModeRef.current = discoverMode;
  const isFirstModeRenderRef = useRef(true);
  // Multi-level undo cache: the last ~5 classified cards, newest first.
  const [history, setHistory] = useState<SwipeHistoryItem[]>([]);
  // Ctrl+Z is a global hotkey with no confirmation and stays armed
  // indefinitely after any decision (until the next one) — an accidental
  // press (e.g. reflexive undo muscle-memory) silently reverts a real
  // decision with zero feedback, which reads as "I already classified this
  // and it keeps reappearing." This message makes every undo (button or
  // hotkey) visible for a few seconds so it's never silent.
  const [undoneMessage, setUndoneMessage] = useState<string | null>(null);
  const undoneMessageTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  useEffect(() => {
    return () => {
      if (undoneMessageTimerRef.current) clearTimeout(undoneMessageTimerRef.current);
    };
  }, []);
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
  const fillQueue = useCallback((): Promise<void> => {
    // One run at a time; callers during a run share its promise, so
    // "await fillQueue()" always means "the queue is as full as it gets".
    if (fillPromiseRef.current) return fillPromiseRef.current;
    const run = async () => {
      // A resetQueue that lands mid-run sets pendingResetRef; the loop then
      // goes again under the new generation inside the SAME run, so waiters
      // never see a half-finished (empty) queue and call the deck exhausted.
      do {
        pendingResetRef.current = false;
        const myGeneration = fillGenerationRef.current;
        for (let round = 0; round < MAX_FILL_ROUNDS && queueRef.current.length < PREFETCH_TARGET; round++) {
          const need = PREFETCH_TARGET - queueRef.current.length;
          const recommended = discoverModeRef.current === "recommended";
          // One call for the whole round: the backend does the per-refill setup once
          // and returns distinct cards, instead of `need` calls repeating it (and each
          // holding the db lock for the whole of it).
          const results = await discoverCatalogCards(recommended, need);
          // resetQueue fired while this round was in flight (e.g. the deck
          // panel just saved new filters) — these results were requested
          // before that, discard them rather than refilling the just-cleared
          // queue with stale-filtered cards.
          if (fillGenerationRef.current !== myGeneration) break;
          const seen = new Set(queueRef.current.map((c) => c.url));
          if (cardUrlRef.current) seen.add(cardUrlRef.current);
          decidedUrlsRef.current.forEach((url) => seen.add(url));
          // Walk results in order, adding each accepted card's url to `seen`
          // as it's accepted — this also dedups two concurrent pickers in the
          // SAME round returning the same not-yet-persisted card.
          const fresh: SwipeCard[] = [];
          for (const c of results) {
            if (!seen.has(c.url)) {
              seen.add(c.url);
              fresh.push(c);
            }
          }
          // No forward progress this round (truly empty, or every hit was a
          // duplicate of something already queued/decided): stop rather than
          // burning more rounds chasing the same not-yet-decided cards.
          if (fresh.length === 0) break;
          queueRef.current = [...queueRef.current, ...fresh];
        }
      } while (pendingResetRef.current);
    };
    const p = run().finally(() => {
      fillPromiseRef.current = null;
    });
    fillPromiseRef.current = p;
    return p;
  }, []);

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
        fillQueue().catch((err) => console.error("fillQueue failed in background refill", err));
      }
      return;
    }
    cardUrlRef.current = null;
    setCard(null);
    setLoading(true);
    fillQueue()
      .then(() => {
        // A resetQueue callback may already have shown a card.
        if (cardUrlRef.current !== null) return;
        if (queueRef.current.length > 0) {
          popNext();
        } else {
          setExhausted(true);
        }
      })
      .catch((err) => {
        console.error("fillQueue failed in popNext", err);
        if (cardUrlRef.current === null && queueRef.current.length === 0) {
          setExhausted(true);
        }
      })
      .finally(() => {
        setLoading(false);
      });
  }, [fillQueue]);

  useEffect(() => {
    let cancelled = false;
    refreshHistory();
    (async () => {
      setLoading(true);
      // Fast path: fetch the first card directly to show it on screen immediately,
      // then top up the rest of the prefetch buffer concurrently in the background.
      const recommended = discoverModeRef.current === "recommended";
      const firstCard = await discoverCatalogCard(recommended).catch(() => null);
      if (cancelled) return;
      if (firstCard && !decidedUrlsRef.current.has(firstCard.url)) {
        cardUrlRef.current = firstCard.url;
        setCard(firstCard);
        setLoading(false);
        setExhausted(false);
        void fillQueue().catch((err) => console.error("fillQueue background fill failed", err));
      } else {
        try {
          await fillQueue();
          if (cancelled) return;
          popNext();
        } catch (err) {
          console.error("fillQueue failed on initial load", err);
          if (!cancelled && cardUrlRef.current === null && queueRef.current.length === 0) {
            setExhausted(true);
          }
        } finally {
          if (!cancelled) {
            setLoading(false);
          }
        }
      }
    })();
    return () => {
      cancelled = true;
    };
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
    fillGenerationRef.current += 1;
    queueRef.current = [];
    // A run already in flight picks up the bump and goes again; otherwise
    // this starts one. Either way, once the queue is refilled, show a card if
    // none is on screen (a deck that had gone "exhausted" revives).
    pendingResetRef.current = true;
    fillQueue()
      .then(() => {
        if (cardUrlRef.current === null && queueRef.current.length > 0) popNext();
      })
      .catch((err) => {
        console.error("fillQueue failed in resetQueue", err);
      });
  }, [fillQueue, popNext]);

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
      const outcome = decidePromise.then(
        (seriesId) => ({ ok: true as const, seriesId }),
        (err) => ({ ok: false as const, err })
      );
      setTimeout(() => {
        setOutDirection(null);
        popNext(); // instant — already prefetched, no round-trip to wait on
        busyRef.current = false;
        outcome.then((res) => {
          if (res.ok) {
            const seriesId = res.seriesId;
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
          } else {
            const err = res.err;
            console.error("decideCatalogCard failed for", activeCard.url, err);
            decidedUrlsRef.current.delete(activeCard.url);
            if (lastDecidedUrlRef.current === activeCard.url) {
              lastDecidedUrlRef.current = null;
            }
            queueRef.current = [activeCard, ...queueRef.current];
            if (cardUrlRef.current === null) {
              popNext();
            }
            setUndoneMessage(t("errors.generic", { detail: String(err) }));
            if (undoneMessageTimerRef.current) clearTimeout(undoneMessageTimerRef.current);
            undoneMessageTimerRef.current = setTimeout(() => setUndoneMessage(null), 5000);
          }
        });
      }, 160);
    },
    [card, popNext, enqueueLink, refreshHistory, t]
  );

  const undo = useCallback(async () => {
    if (!canUndo) return;
    setCanUndo(false);
    // The backend reports what it actually reversed. Reading history[0]
    // instead (what this used to do) named the wrong card whenever the real
    // one had been merged into an existing series by the background link —
    // that row is gone, so the strip doesn't list it — and claimed a
    // successful undo even when nothing had been undone at all.
    const undoneTitle = await undoLastSwipe();
    // The card is back in the deck (its series row was hard-deleted, or the
    // flags a merge set on the surviving row were cleared) — let fillQueue
    // serve it again. Only when something really was undone: otherwise the
    // decision still stands and the card must stay out of the deck.
    if (undoneTitle && lastDecidedUrlRef.current) {
      decidedUrlsRef.current.delete(lastDecidedUrlRef.current);
      lastDecidedUrlRef.current = null;
    }
    if (undoneTitle) {
      setUndoneMessage(t("discover.undone", { title: undoneTitle }));
      if (undoneMessageTimerRef.current) clearTimeout(undoneMessageTimerRef.current);
      undoneMessageTimerRef.current = setTimeout(() => setUndoneMessage(null), 5000);
    }
    refreshHistory();
  }, [canUndo, refreshHistory, t]);

  // History-strip actions. Re-classifying a past card moves it between lists
  // (local, via the shared reclassify inverse); returning it to the deck
  // hard-deletes its row so the picker offers it again.
  //
  // Both DISARM the undo button. `canUndo` only ever means "the backend's
  // swipe_history still starts with the decision this button claims to
  // reverse", and it stops meaning that the moment any history entry other
  // than that one is consumed or altered: undo_last_swipe always pops the
  // FRONT of the backend deque, so after a "Devolver al mazo" on an older
  // entry (which removes it from the deque) a subsequent Ctrl+Z would
  // hard-revert whatever decision happens to be at the front now — a
  // different card the user never asked to undo, with no confirmation.
  // Re-arming is deliberately left to the next real decide(): "make a new
  // decision before undo works again" is correct and cheap, whereas trying
  // to keep a stale `true` accurate would need the backend to expose its
  // current undo target.
  const reclassifyHistory = useCallback(
    async (item: SwipeHistoryItem, action: "discard" | "want" | "seen") => {
      setCanUndo(false);
      await reclassifySeries(item.series_id, RECLASSIFY_TARGET[action]);
      refreshHistory();
    },
    [refreshHistory]
  );
  const returnToDeck = useCallback(
    async (item: SwipeHistoryItem) => {
      setCanUndo(false);
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
      // Don't hijack arrow keys (or Ctrl+Z) while the user is focused on a
      // real form control — the filters panel's date inputs use ArrowLeft/
      // Right/Up/Down to move between day/month/year segments, and without
      // this check this window-level listener wins the race and fires a
      // real swipe decision on the current card instead.
      const el = document.activeElement;
      const tag = el?.tagName;
      if (tag === "INPUT" || tag === "TEXTAREA" || tag === "SELECT" || (el as HTMLElement | null)?.isContentEditable) {
        return;
      }
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

  // The sheet acts on the card on screen (it stays current while the sheet is open).
  const sheet = useAnimeSheet({
    // Only when the sheet still shows the card on screen.
    onWant: (a) => {
      if (card && (card.catalog?.id ?? anilistIdFromUrl(card.url)) === a.id) decide("Want", "want");
    },
    onSeen: (a) => {
      if (card && (card.catalog?.id ?? anilistIdFromUrl(card.url)) === a.id) decide("Seen", "seen");
    },
  });

  function openSheet(c: SwipeCard) {
    const id = c.catalog?.id ?? anilistIdFromUrl(c.url);
    if (id == null) {
      openEpisode(c.url).catch((err) => console.error("openEpisode failed for", c.url, err));
      return;
    }
    sheet.open(
      c.catalog ?? {
        id,
        title: c.title,
        cover_url: c.poster_url,
        url: c.url,
        format: c.kind || null,
        genres: c.matched_genre ? [c.matched_genre] : [],
      }
    );
  }

  const synopsis = useCardSynopsis(card);
  const info = card ? deckCardInfo(card, t) : null;

  return (
    <DeckPanel onBansSaved={resetQueue}>
      {({ openSettings, activeBans }) => (
        <>
          {sheet.element}
          {exhausted ? (
            <div className="disc-empty">
              <h3 className="disc-empty-title">{t("discover.exhaustedTitle")}</h3>
              <p className="muted">
                {activeBans > 0 ? t("discover.exhaustedFiltered", { count: activeBans }) : t("discover.exhausted")}
              </p>
              <div className="disc-empty-actions">
                <button type="button" className="btn btn-primary" onClick={openSettings}>
                  {t("discover.deckSettings")}
                </button>
                {discoverMode === "recommended" && (
                  <button type="button" className="btn" onClick={() => setDiscoverMode("random")}>
                    {t("discover.tryRandom")}
                  </button>
                )}
              </div>
            </div>
          ) : (
            <section className="disc-stage" aria-label={t("discover.stageAria")}>
              <div className="disc-deck">
                <div className="disc-ghost disc-ghost-2" aria-hidden="true" />
                <div className="disc-ghost disc-ghost-1" aria-hidden="true" />
                {card ? (
                  <button
                    key={card.url}
                    type="button"
                    className={`disc-card${outDirection ? ` swipe-out-${outDirection}` : ""}`}
                    title={t("discover.openPageTitle")}
                    onClick={() => openSheet(card)}
                  >
                    {(() => {
                      const src = resolveCoverUrl(card.poster_url);
                      return src ? <img src={src} alt="" /> : <span className="disc-card-fallback">{card.title}</span>;
                    })()}
                    {card.kind && <span className="chip">{formatLabel(t, card.kind)}</span>}
                    {info?.score && <span className="disc-card-score">{info.score}</span>}
                  </button>
                ) : (
                  <div className="disc-card disc-card-loading" aria-busy="true" />
                )}
              </div>

              <div className="disc-info">
                {card && info ? (
                  <>
                    {card.matched_genre && (
                      <p className="disc-why">
                        {discoverMode === "recommended"
                          ? t("discover.whyRecommended", { genre: card.matched_genre })
                          : t("discover.whyRandom", { genre: card.matched_genre })}
                      </p>
                    )}
                    <h2 className="disc-title">{card.title}</h2>
                    {info.alt && <p className="disc-alt">{info.alt}</p>}
                    {info.facts && <p className="disc-facts">{info.facts}</p>}
                    {info.genres.length > 0 && (
                      <ul className="disc-genres" aria-label={t("animeSheet.genres")}>
                        {info.genres.map((g) => (
                          <li key={g} className="tag">
                            {g}
                          </li>
                        ))}
                      </ul>
                    )}
                    <p className={`disc-synopsis${synopsis === null ? " loading" : ""}`}>
                      {synopsis === null ? t("common.loading") : synopsis || t("discover.noSynopsis")}
                    </p>
                    <button type="button" className="btn-text disc-more" onClick={() => openSheet(card)}>
                      {t("discover.fullSheet")}
                    </button>
                  </>
                ) : (
                  <p className="muted">{loading ? t("common.loading") : ""}</p>
                )}

                <div className="disc-actions">
                  <button
                    type="button"
                    className="disc-act disc-act-discard"
                    onClick={() => decide("Discard", "discard")}
                    disabled={!card}
                  >
                    {t("discover.discard")}
                    <span className="disc-act-key">{t("discover.keyLeft")}</span>
                  </button>
                  <button
                    type="button"
                    className="disc-act disc-act-seen"
                    onClick={() => decide("Seen", "seen")}
                    disabled={!card}
                  >
                    {t("discover.seen")}
                    <span className="disc-act-key">{t("discover.keyUp")}</span>
                  </button>
                  <button
                    type="button"
                    className="disc-act disc-act-want"
                    onClick={() => decide("Want", "want")}
                    disabled={!card}
                  >
                    {t("discover.want")}
                    <span className="disc-act-key">{t("discover.keyRight")}</span>
                  </button>
                  <button
                    type="button"
                    className="disc-act disc-act-undo"
                    onClick={undo}
                    disabled={!canUndo}
                    aria-label={t("discover.undoTitle")}
                    title={t("discover.undoTitle")}
                  >
                    ↺
                  </button>
                </div>
              </div>
            </section>
          )}

          {(undoneMessage || linkStatuses.length > 0) && (
            <div className="disc-status" role="status">
              {undoneMessage && <div className="muted text-sm text-danger">{undoneMessage}</div>}
              {linkStatuses.map((s) => (
                <div key={s.id} className="muted text-sm">
                  {s.state === "searching" && t("discover.searching", { title: s.title })}
                  {s.state === "linked" && t("discover.linked", { count: s.episodes ?? 0 })}
                  {s.state === "nomatch" && t("discover.noMatch", { title: s.title })}
                </div>
              ))}
            </div>
          )}

          {history.length > 0 && (
            <section className="disc-recent" aria-labelledby="disc-recent-title">
              <div className="cat-shelf-head">
                <h2 id="disc-recent-title" className="cat-shelf-title">
                  {t("discover.historyHeading")}
                </h2>
                <span className="cat-shelf-hint">{t("discover.historyHint")}</span>
              </div>
              <div className="disc-recent-track">
                {history.map((h) => (
                  <HistoryRow key={h.series_id} item={h} onReclassify={reclassifyHistory} onReturn={returnToDeck} />
                ))}
              </div>
            </section>
          )}
        </>
      )}
    </DeckPanel>
  );
}
