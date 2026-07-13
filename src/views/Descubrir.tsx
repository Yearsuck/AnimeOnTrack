import { useCallback, useEffect, useRef, useState } from "react";
import {
  decideCatalogCard,
  deleteSeries,
  discoverCatalogCard,
  getCatalogFacets,
  getDeckBans,
  getSeriesGenres,
  getTopGenres,
  linkCatalogSeries,
  listBacklog,
  listSwipeHistory,
  listWatchedExternally,
  openEpisode,
  promoteDiscarded,
  reclassifySeries,
  setBacklogStatus,
  setDeckBans,
  startWatching,
  undoLastSwipe,
  undoSwipeEntry,
} from "../api";
import { useT } from "../i18n";
import { categoryColor } from "../lib/categoryColor";
import { isUnlinkedCatalogRow } from "../lib/catalogLink";
import type {
  Classification,
  GenreAffinity,
  Series,
  SwipeCard,
  SwipeDecision,
  SwipeHistoryItem,
} from "../types";

// The deck's format whitelist (backend DEFAULT_FORMATS in
// db::random_catalog_anime_in_genre) — the "tipos" the Filtros view can ban.
const DECK_FORMATS = ["TV", "MOVIE", "OVA", "ONA", "SPECIAL"];

// Map a history-strip re-classify action to its reclassify_series target.
const RECLASSIFY_TARGET: Record<"discard" | "want" | "seen", Classification> = {
  discard: "Discarded",
  want: "Want",
  seen: "WatchedExternally",
};

type LinkStatus = {
  id: number;
  title: string;
  state: "searching" | "linked" | "nomatch";
  episodes?: number;
};

/// Serializes `linkCatalogSeries` calls through a single promise chain so
/// rapid swiping never spawns unbounded parallel scrapes — the backend's
/// SCRAPE_PERMITS semaphore would otherwise just queue them behind whatever
/// the deck's own prefetching needs, defeating the point of not blocking the
/// swipe on the link. Keeps the most recent 2 statuses for display.
function useLinkQueue() {
  const chainRef = useRef<Promise<void>>(Promise.resolve());
  const [statuses, setStatuses] = useState<LinkStatus[]>([]);

  const upsert = useCallback((next: LinkStatus) => {
    setStatuses((prev) => [next, ...prev.filter((s) => s.id !== next.id)].slice(0, 2));
  }, []);

  const enqueue = useCallback(
    (seriesId: number, title: string) => {
      upsert({ id: seriesId, title, state: "searching" });
      chainRef.current = chainRef.current
        .then(() => linkCatalogSeries(seriesId))
        .then((outcome) => {
          if (outcome.type === "Linked") {
            upsert({ id: seriesId, title, state: "linked", episodes: outcome.episodes });
          } else if (outcome.type === "NoMatch") {
            upsert({ id: seriesId, title, state: "nomatch" });
          }
          // AlreadyLinked: nothing to show — a freshly-decided card is never
          // already linked, this only matters for the manual retry button.
        })
        .catch((err) => {
          console.error("linkCatalogSeries failed for", seriesId, err);
          upsert({ id: seriesId, title, state: "nomatch" });
        });
    },
    [upsert]
  );

  return { statuses, enqueue };
}

type SubView = "swipe" | "listas" | "filtros";
type SwipeOutDirection = "discard" | "want" | "seen" | null;

// AniList's siteUrl shape is https://anilist.co/anime/{id}/{slug} — catalog
// cards carry that as their url, this pulls the id back out for
// decide_catalog_card (which needs it to key the synthetic series row).
function anilistIdFromUrl(url: string): number | null {
  const m = url.match(/anilist\.co\/anime\/(\d+)/);
  return m ? Number(m[1]) : null;
}

const TOP_GENRES_LIMIT = 5;
// Prefetch buffer: keep this many cards ready locally so a decision shows
// the next card instantly instead of waiting on a fresh discover_catalog_card
// round-trip. discover_catalog_card is a local SQLite read (no Cloudflare
// exposure, no network at all), so filling this concurrently is cheap and
// safe — unlike the scraped-site path, there's no rate limit to respect here.
const PREFETCH_TARGET = 10;
const REFILL_THRESHOLD = 4;
const MAX_FILL_ROUNDS = 5;

function TasteChips() {
  const t = useT();
  const [genres, setGenres] = useState<GenreAffinity[]>([]);
  useEffect(() => {
    getTopGenres(TOP_GENRES_LIMIT).then(setGenres);
  }, []);
  if (genres.length === 0) return null;
  return (
    <div style={{ display: "flex", alignItems: "center", gap: 8, flexWrap: "wrap", marginBottom: 16 }}>
      <span className="muted" style={{ fontSize: 12 }}>
        {t("discover.topGenres")}
      </span>
      {genres.map((g) => (
        <span
          key={g.genre}
          style={{
            fontSize: 12,
            fontWeight: 600,
            padding: "3px 10px",
            borderRadius: "var(--radius-round)",
            background: categoryColor(g.genre),
            color: "#05121f",
          }}
        >
          {g.genre}
        </span>
      ))}
    </div>
  );
}

function SwipeView() {
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
        const results = await Promise.all(Array.from({ length: need }, () => discoverCatalogCard()));
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
  );
}

const DECISION_BADGE = {
  want: "discover.badgeWant",
  seen: "discover.badgeSeen",
  discard: "discover.badgeDiscard",
  none: "discover.badgeDiscard",
} as const;

// One row of the swipe-history strip: poster, title, current-decision badge,
// quick re-classify buttons, and a "return to deck" undo for this one card.
function HistoryRow({
  item,
  onReclassify,
  onReturn,
}: {
  item: SwipeHistoryItem;
  onReclassify: (item: SwipeHistoryItem, action: "discard" | "want" | "seen") => void;
  onReturn: (item: SwipeHistoryItem) => void;
}) {
  const t = useT();
  return (
    <div className="swipe-history-row">
      {item.poster_url && <img src={item.poster_url} alt="" />}
      <div className="swipe-history-main">
        <div className="swipe-history-title" title={item.title}>
          {item.title}
        </div>
        <div className="muted" style={{ fontSize: 11 }}>
          {t(DECISION_BADGE[item.decision])}
        </div>
      </div>
      <div className="swipe-history-actions">
        <button
          className="btn btn-ghost"
          title={t("discover.discard")}
          onClick={() => onReclassify(item, "discard")}
          disabled={item.decision === "discard"}
        >
          ✕
        </button>
        <button
          className="btn btn-ghost"
          title={t("discover.want")}
          onClick={() => onReclassify(item, "want")}
          disabled={item.decision === "want"}
        >
          ★
        </button>
        <button
          className="btn btn-ghost"
          title={t("discover.seen")}
          onClick={() => onReclassify(item, "seen")}
          disabled={item.decision === "seen"}
        >
          ✓
        </button>
        <button
          className="btn btn-ghost"
          title={t("discover.returnToDeck")}
          onClick={() => onReturn(item)}
        >
          ↺
        </button>
      </div>
    </div>
  );
}

// ---- Shared Listas card bits (poster+initials fallback, status chip, and an
// outside-click overflow menu — same visual language as the Library cards). ----

function listasInitials(title: string): string {
  const chars = title
    .trim()
    .split(/\s+/)
    .filter(Boolean)
    .slice(0, 2)
    .map((w) => w[0]?.toUpperCase() ?? "")
    .join("");
  return chars || "?";
}

function PosterThumb({ series }: { series: Series }) {
  const [failed, setFailed] = useState(false);
  const showFallback = !series.cover_url || failed;
  return (
    <div className="listas-poster">
      {showFallback ? (
        <div className="poster-fallback" aria-hidden="true">
          {listasInitials(series.title)}
        </div>
      ) : (
        <img src={series.cover_url!} alt="" loading="lazy" onError={() => setFailed(true)} />
      )}
    </div>
  );
}

function OverflowMenu({
  label,
  items,
}: {
  label: string;
  items: { label: string; onClick: () => void }[];
}) {
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!open) return;
    function onDown(e: MouseEvent) {
      if (ref.current && !ref.current.contains(e.target as Node)) setOpen(false);
    }
    document.addEventListener("mousedown", onDown);
    return () => document.removeEventListener("mousedown", onDown);
  }, [open]);
  if (items.length === 0) return null;
  return (
    <div className="card-menu-wrap" ref={ref}>
      <button
        type="button"
        className="card-menu-btn"
        aria-label={label}
        onClick={() => setOpen((v) => !v)}
      >
        ⋯
      </button>
      {open && (
        <div className="card-menu-list">
          {items.map((it, i) => (
            <button
              key={i}
              type="button"
              className="card-menu-item"
              onClick={() => {
                setOpen(false);
                it.onClick();
              }}
            >
              {it.label}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}

function WantRow({
  series,
  onChanged,
  onOpenSeries,
}: {
  series: Series;
  onChanged: () => void;
  onOpenSeries: (s: Series) => void;
}) {
  const t = useT();
  const [genres, setGenres] = useState<string[]>([]);
  const [linking, setLinking] = useState(false);
  const [starting, setStarting] = useState(false);
  const [noMatch, setNoMatch] = useState(false);
  const unlinked = isUnlinkedCatalogRow(series);
  useEffect(() => {
    getSeriesGenres(series.id).then(setGenres);
  }, [series.id]);

  const retryLink = async () => {
    setLinking(true);
    try {
      await linkCatalogSeries(series.id);
    } finally {
      setLinking(false);
      onChanged();
    }
  };

  // start_watching links an unlinked catalog row (searches the site) before
  // marking it followed -- the button-press trigger from the design spec,
  // so unlike the swipe-to-Seen path it awaits and shows a spinner. A
  // NoMatch keeps the row in "want" (backend never sets followed) and
  // surfaces the same "not found" message the retry button would show.
  const handleStartWatching = async () => {
    setStarting(true);
    setNoMatch(false);
    try {
      const outcome = await startWatching(series.id);
      if (outcome.type === "NoMatch") {
        setNoMatch(true);
      } else {
        onChanged();
      }
    } finally {
      setStarting(false);
    }
  };

  return (
    <div className="listas-card">
      <PosterThumb series={series} />
      <div className="backlog-main">
        <div className="listas-card-head">
          <span
            className="listas-title"
            onClick={() => onOpenSeries(series)}
            style={{ cursor: "pointer" }}
            title={t("discover.viewDetailsTitle")}
          >
            {series.title}
          </span>
          <span className="listas-chip listas-chip-want">{t("discover.chipWant")}</span>
        </div>
        {genres.length > 0 && <div className="backlog-genres">{genres.join(", ")}</div>}
        {unlinked && (
          <div className="muted" style={{ fontSize: 11 }} title={t("discover.notFoundYetTitle")}>
            {t("discover.unlinked")}
          </div>
        )}
        {noMatch && (
          <div className="muted" style={{ fontSize: 11, color: "var(--danger)" }}>
            {t("discover.notFoundInSite")}
          </div>
        )}
      </div>
      <div className="listas-actions">
        <button className="btn btn-primary" onClick={handleStartWatching} disabled={starting}>
          {starting ? t("discover.searchingBtn") : t("discover.startWatching")}
        </button>
        <OverflowMenu
          label={t("discover.moreActions")}
          items={[
            {
              label: t("discover.discard"),
              onClick: async () => {
                await setBacklogStatus(series.id, "discarded");
                onChanged();
              },
            },
            {
              label: t("discover.removeFromList"),
              onClick: async () => {
                await reclassifySeries(series.id, "None");
                onChanged();
              },
            },
            ...(unlinked
              ? [
                  {
                    label: linking ? t("discover.searchingBtn") : t("discover.searchOnSite"),
                    onClick: retryLink,
                  },
                ]
              : []),
            { label: t("discover.viewDetails"), onClick: () => onOpenSeries(series) },
          ]}
        />
      </div>
    </div>
  );
}

function WatchedRow({ series, onChanged }: { series: Series; onChanged: () => void }) {
  const t = useT();
  return (
    <div className="listas-card">
      <PosterThumb series={series} />
      <div className="backlog-main">
        <div className="listas-card-head">
          <span className="listas-title">{series.title}</span>
          <span className="listas-chip listas-chip-watched">{t("discover.chipWatched")}</span>
        </div>
      </div>
      <div className="listas-actions">
        <button
          className="btn"
          onClick={async () => {
            await reclassifySeries(series.id, "None");
            onChanged();
          }}
        >
          {t("discover.markUnwatched")}
        </button>
        <OverflowMenu
          label={t("discover.moreActions")}
          items={[
            {
              label: t("discover.moveToWantFromWatched"),
              onClick: async () => {
                await reclassifySeries(series.id, "Want");
                onChanged();
              },
            },
          ]}
        />
      </div>
    </div>
  );
}

function DiscardedRow({ series, onChanged }: { series: Series; onChanged: () => void }) {
  const t = useT();
  return (
    <div className="listas-card">
      <PosterThumb series={series} />
      <div className="backlog-main">
        <div className="listas-card-head">
          <span className="listas-title">{series.title}</span>
          <span className="listas-chip listas-chip-discarded">{t("discover.chipDiscarded")}</span>
        </div>
      </div>
      <div className="listas-actions">
        <button
          className="btn btn-primary"
          onClick={async () => {
            await promoteDiscarded(series.id);
            onChanged();
          }}
        >
          {t("discover.moveToWant")}
        </button>
        <OverflowMenu
          label={t("discover.moreActions")}
          items={[
            {
              label: t("discover.deleteCompletely"),
              onClick: async () => {
                await deleteSeries(series.id);
                onChanged();
              },
            },
          ]}
        />
      </div>
    </div>
  );
}

type ListTab = "want" | "discarded" | "watched";

function ListasView({ onOpenSeries }: { onOpenSeries: (s: Series) => void }) {
  const t = useT();
  const [want, setWant] = useState<Series[]>([]);
  const [discarded, setDiscarded] = useState<Series[]>([]);
  const [watched, setWatched] = useState<Series[]>([]);
  const [tab, setTab] = useState<ListTab>("want");

  const load = useCallback(async () => {
    const [w, d, wa] = await Promise.all([
      listBacklog("want"),
      listBacklog("discarded"),
      listWatchedExternally(),
    ]);
    setWant(w);
    setDiscarded(d);
    setWatched(wa);
  }, []);

  useEffect(() => {
    load();
  }, [load]);

  const tabs: { key: ListTab; label: string; count: number }[] = [
    { key: "want", label: t("discover.wantHeading"), count: want.length },
    { key: "discarded", label: t("discover.discardedHeading"), count: discarded.length },
    { key: "watched", label: t("discover.watchedHeading"), count: watched.length },
  ];

  return (
    <>
      <div className="seg listas-seg" role="tablist">
        {tabs.map((tb) => (
          <button
            key={tb.key}
            type="button"
            role="tab"
            aria-selected={tab === tb.key}
            className={`seg-btn${tab === tb.key ? " active" : ""}`}
            onClick={() => setTab(tb.key)}
          >
            {tb.label}
            <span className="listas-seg-count">{tb.count}</span>
          </button>
        ))}
      </div>

      {tab === "want" &&
        (want.length === 0 ? (
          <div className="empty">{t("discover.wantEmpty")}</div>
        ) : (
          <div className="listas-grid">
            {want.map((s) => (
              <WantRow key={s.id} series={s} onChanged={load} onOpenSeries={onOpenSeries} />
            ))}
          </div>
        ))}

      {tab === "discarded" &&
        (discarded.length === 0 ? (
          <div className="empty">{t("discover.discardedEmpty")}</div>
        ) : (
          <div className="listas-grid">
            {discarded.map((s) => (
              <DiscardedRow key={s.id} series={s} onChanged={load} />
            ))}
          </div>
        ))}

      {tab === "watched" &&
        (watched.length === 0 ? (
          <div className="empty">{t("discover.watchedEmpty")}</div>
        ) : (
          <div className="listas-grid">
            {watched.map((s) => (
              <WatchedRow key={s.id} series={s} onChanged={load} />
            ))}
          </div>
        ))}
    </>
  );
}

// Deck genre/type bans (Section B). Banned chips render "active" (highlighted
// = excluded from the deck). Genres come from the synced catalog facets;
// formats are the fixed deck whitelist. Persisted via setDeckBans; takes
// effect on the next discover_catalog_card.
function FiltersView() {
  const t = useT();
  const [allGenres, setAllGenres] = useState<string[]>([]);
  const [bannedGenres, setBannedGenres] = useState<Set<string>>(new Set());
  const [bannedFormats, setBannedFormats] = useState<Set<string>>(new Set());
  const [saving, setSaving] = useState(false);
  const [saved, setSaved] = useState(false);

  useEffect(() => {
    getDeckBans()
      .then((b) => {
        setBannedGenres(new Set(b.genres));
        setBannedFormats(new Set(b.formats));
      })
      .catch((err) => console.error("getDeckBans failed", err));
    getCatalogFacets()
      .then((f) => setAllGenres(f.genres))
      .catch((err) => console.error("getCatalogFacets failed", err));
  }, []);

  function toggle(set: Set<string>, setter: (s: Set<string>) => void, value: string) {
    const next = new Set(set);
    if (next.has(value)) next.delete(value);
    else next.add(value);
    setter(next);
    setSaved(false);
  }

  async function save() {
    setSaving(true);
    try {
      await setDeckBans([...bannedGenres], [...bannedFormats]);
      setSaved(true);
    } finally {
      setSaving(false);
    }
  }

  return (
    <div className="series-block" style={{ padding: 16 }}>
      <p className="muted" style={{ marginTop: 0, fontSize: 12.5 }}>
        {t("discover.filtersIntro")}
      </p>

      <h3 className="card-title" style={{ marginBottom: 8 }}>
        {t("discover.filtersFormatsHeading")}
      </h3>
      <div className="chip-row" style={{ marginBottom: 16 }}>
        {DECK_FORMATS.map((f) => (
          <button
            key={f}
            type="button"
            className={`chip-toggle${bannedFormats.has(f) ? " active" : ""}`}
            onClick={() => toggle(bannedFormats, setBannedFormats, f)}
          >
            {f}
          </button>
        ))}
      </div>

      <h3 className="card-title" style={{ marginBottom: 8 }}>
        {t("discover.filtersGenresHeading")}
      </h3>
      {allGenres.length > 0 && (
        <div className="chip-row" style={{ marginBottom: 16 }}>
          {allGenres.map((g) => (
            <button
              key={g}
              type="button"
              className={`chip-toggle${bannedGenres.has(g) ? " active" : ""}`}
              onClick={() => toggle(bannedGenres, setBannedGenres, g)}
            >
              {g}
            </button>
          ))}
        </div>
      )}

      <div className="row" style={{ alignItems: "center", gap: 12 }}>
        <button className="btn btn-primary" onClick={save} disabled={saving}>
          {saving ? t("discover.filtersSaving") : t("discover.filtersSave")}
        </button>
        {saved && <span className="muted">{t("discover.filtersSaved")}</span>}
      </div>
    </div>
  );
}

// onOpenSeries is the same App.tsx-owned navigation callback Pending/Library/
// AiringGrid already use to open SeriesDetail. Wiring it into Listas' "Quiero
// ver" rows is what makes trigger (c) from the design spec ("SeriesDetail
// opened on an unlinked catalog row" — see SeriesDetail.tsx's link-on-open
// effect) actually reachable: without a click-through here, an unfollowed
// catalog row had no UI path into SeriesDetail at all.
export function Descubrir({ onOpenSeries }: { onOpenSeries: (s: Series) => void }) {
  const t = useT();
  const [subView, setSubView] = useState<SubView>("swipe");

  return (
    <div className="page">
      <div className="page-head">
        <h2 className="page-title">{t("nav.discover")}</h2>
      </div>
      <div className="tabs" style={{ marginBottom: 20 }}>
        <button
          className={`tab ${subView === "swipe" ? "active" : ""}`}
          onClick={() => setSubView("swipe")}
        >
          {t("discover.tabSwipe")}
        </button>
        <button
          className={`tab ${subView === "listas" ? "active" : ""}`}
          onClick={() => setSubView("listas")}
        >
          {t("discover.tabLists")}
        </button>
        <button
          className={`tab ${subView === "filtros" ? "active" : ""}`}
          onClick={() => setSubView("filtros")}
        >
          {t("discover.tabFilters")}
        </button>
      </div>

      {subView === "swipe" && <SwipeView />}
      {subView === "listas" && <ListasView onOpenSeries={onOpenSeries} />}
      {subView === "filtros" && <FiltersView />}
    </div>
  );
}
