import { useCallback, useEffect, useRef, useState } from "react";
import {
  decideSwipe,
  deleteSeries,
  discoverSwipeCard,
  getSeriesGenres,
  getTopGenres,
  listBacklog,
  openEpisode,
  promoteDiscarded,
  setBacklogStatus,
  startWatching,
  undoLastSwipe,
} from "../api";
import { categoryColor } from "../lib/categoryColor";
import type { GenreAffinity, Series, SwipeCard, SwipeDecision } from "../types";

type SubView = "swipe" | "listas";
type SwipeOutDirection = "discard" | "want" | "seen" | null;

const TOP_GENRES_LIMIT = 5;
// Prefetch buffer: keep this many cards ready locally so a decision shows
// the next card instantly instead of waiting on a fresh discover_swipe_card
// round-trip (that command re-picks a random genre/page most of the time,
// so it's a real network fetch far more often than the "~1 fetch per 10
// swipes" the buffer keying alone provides). Backend scrape fetches are
// still globally capped at 2 concurrent (see scraper_engine.rs), so firing
// several discover_swipe_card calls at once to fill this is safe — they
// just queue up server-side instead of stacking on top of each other.
const PREFETCH_TARGET = 10;
const REFILL_THRESHOLD = 4;
const MAX_FILL_ROUNDS = 5;

function TasteChips() {
  const [genres, setGenres] = useState<GenreAffinity[]>([]);
  useEffect(() => {
    getTopGenres(TOP_GENRES_LIMIT).then(setGenres);
  }, []);
  if (genres.length === 0) return null;
  return (
    <div style={{ display: "flex", alignItems: "center", gap: 8, flexWrap: "wrap", marginBottom: 16 }}>
      <span className="muted" style={{ fontSize: 12 }}>
        Tus géneros favoritos:
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
  const [card, setCard] = useState<SwipeCard | null>(null);
  const [outDirection, setOutDirection] = useState<SwipeOutDirection>(null);
  const [canUndo, setCanUndo] = useState(false);
  const [exhausted, setExhausted] = useState(false);
  const [loading, setLoading] = useState(true);
  const busyRef = useRef(false);
  const queueRef = useRef<SwipeCard[]>([]);
  const fillingRef = useRef(false);
  const cardUrlRef = useRef<string | null>(null);

  // Top the local queue back up to PREFETCH_TARGET, deduping against
  // whatever's already queued or on screen (discover_swipe_card can hand
  // out the same not-yet-decided card twice — it only excludes cards
  // already persisted to the DB, and a prefetched card isn't persisted
  // until the user actually decides on it).
  const fillQueue = useCallback(async () => {
    if (fillingRef.current) return;
    fillingRef.current = true;
    try {
      for (let round = 0; round < MAX_FILL_ROUNDS && queueRef.current.length < PREFETCH_TARGET; round++) {
        const need = PREFETCH_TARGET - queueRef.current.length;
        const results = await Promise.all(Array.from({ length: need }, () => discoverSwipeCard()));
        const seen = new Set(queueRef.current.map((c) => c.url));
        if (cardUrlRef.current) seen.add(cardUrlRef.current);
        const fresh = results.filter((c): c is SwipeCard => c !== null && !seen.has(c.url));
        // No forward progress this round (either truly empty, or every hit
        // was a duplicate of something already queued) — stop rather than
        // burning more rounds of concurrent fetches chasing the same
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
      await fillQueue();
      popNext();
      setLoading(false);
    })();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const decide = useCallback(
    async (decision: SwipeDecision, direction: Exclude<SwipeOutDirection, null>) => {
      if (!card || busyRef.current) return;
      busyRef.current = true;
      const url = card.url;
      setOutDirection(direction);
      setCanUndo(false);
      const decidePromise = decideSwipe(url, decision);
      setTimeout(() => {
        setOutDirection(null);
        popNext(); // instant — already prefetched, no round-trip to wait on
        busyRef.current = false;
        decidePromise.then(() => setCanUndo(true));
      }, 160);
    },
    [card, popNext]
  );

  const undo = useCallback(async () => {
    if (!canUndo) return;
    setCanUndo(false);
    await undoLastSwipe();
  }, [canUndo]);

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

  if (exhausted) {
    return (
      <div className="empty">
        No se han encontrado más animes por ahora, prueba más tarde.
      </div>
    );
  }

  return (
    <div className="swipe-stage">
      <TasteChips />
      {card ? (
        <div className={`card swipe-card ${outDirection ? `swipe-out-${outDirection}` : ""}`}>
          <div
            className="poster"
            style={{ cursor: "pointer" }}
            title="Abrir página del anime"
            onClick={() => openEpisode(card.url)}
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
                Género: {card.matched_genre}
              </div>
            )}
          </div>
        </div>
      ) : (
        <div className="card swipe-card">
          <div className="poster" />
          <div className="card-body">
            <div className="muted">{loading ? "Cargando…" : ""}</div>
          </div>
        </div>
      )}

      <div className="swipe-actions">
        <button
          className="btn btn-discard"
          title="Descartar (←)"
          onClick={() => decide("Discard", "discard")}
          disabled={!card}
        >
          ✕
        </button>
        <button
          className="btn btn-want"
          title="Quiero ver (→)"
          onClick={() => decide("Want", "want")}
          disabled={!card}
        >
          ★
        </button>
        <button
          className="btn btn-success"
          title="Ya lo vi (↑)"
          onClick={() => decide("Seen", "seen")}
          disabled={!card}
        >
          ✓
        </button>
        <button
          className="btn btn-ghost"
          title="Deshacer (Ctrl+Z)"
          onClick={undo}
          disabled={!canUndo}
        >
          ↺
        </button>
      </div>
      <div className="swipe-hint">← Descartar · ↑ Ya lo vi · → Quiero ver · Ctrl+Z Deshacer</div>
    </div>
  );
}

function WantRow({ series, onChanged }: { series: Series; onChanged: () => void }) {
  const [genres, setGenres] = useState<string[]>([]);
  useEffect(() => {
    getSeriesGenres(series.id).then(setGenres);
  }, [series.id]);

  return (
    <div className="backlog-row">
      {series.cover_url && <img src={series.cover_url} alt="" />}
      <div className="backlog-main">
        <div className="backlog-title">{series.title}</div>
        <div className="backlog-genres">{genres.join(", ") || " "}</div>
      </div>
      <div className="backlog-actions">
        <button
          className="btn btn-primary"
          onClick={async () => {
            await startWatching(series.id);
            onChanged();
          }}
        >
          Empezar a ver
        </button>
        <button
          className="btn btn-ghost"
          onClick={async () => {
            await setBacklogStatus(series.id, "discarded");
            onChanged();
          }}
        >
          Descartar
        </button>
      </div>
    </div>
  );
}

function DiscardedRow({ series, onChanged }: { series: Series; onChanged: () => void }) {
  return (
    <div className="backlog-row">
      {series.cover_url && <img src={series.cover_url} alt="" />}
      <div className="backlog-main">
        <div className="backlog-title">{series.title}</div>
      </div>
      <div className="backlog-actions">
        <button
          className="btn btn-ghost"
          onClick={async () => {
            await promoteDiscarded(series.id);
            onChanged();
          }}
        >
          Mover a quiero ver
        </button>
        <button
          className="btn btn-ghost"
          onClick={async () => {
            await deleteSeries(series.id);
            onChanged();
          }}
        >
          Eliminar del todo
        </button>
      </div>
    </div>
  );
}

function ListasView() {
  const [want, setWant] = useState<Series[]>([]);
  const [discarded, setDiscarded] = useState<Series[]>([]);

  const load = useCallback(async () => {
    const [w, d] = await Promise.all([listBacklog("want"), listBacklog("discarded")]);
    setWant(w);
    setDiscarded(d);
  }, []);

  useEffect(() => {
    load();
  }, [load]);

  return (
    <>
      <div className="series-block" style={{ marginBottom: 20 }}>
        <div className="series-head">
          <h3 className="card-title">Quiero ver</h3>
        </div>
        {want.length === 0 ? (
          <div className="empty">Nada por aquí todavía.</div>
        ) : (
          want.map((s) => <WantRow key={s.id} series={s} onChanged={load} />)
        )}
      </div>

      <div className="series-block">
        <div className="series-head">
          <h3 className="card-title">Descartados</h3>
        </div>
        {discarded.length === 0 ? (
          <div className="empty">Nada descartado todavía.</div>
        ) : (
          discarded.map((s) => <DiscardedRow key={s.id} series={s} onChanged={load} />)
        )}
      </div>
    </>
  );
}

export function Descubrir() {
  const [subView, setSubView] = useState<SubView>("swipe");

  return (
    <div className="page">
      <div className="page-head">
        <h2 className="page-title">Descubrir</h2>
      </div>
      <div className="tabs" style={{ marginBottom: 20 }}>
        <button
          className={`tab ${subView === "swipe" ? "active" : ""}`}
          onClick={() => setSubView("swipe")}
        >
          Swipe
        </button>
        <button
          className={`tab ${subView === "listas" ? "active" : ""}`}
          onClick={() => setSubView("listas")}
        >
          Listas
        </button>
      </div>

      {subView === "swipe" ? <SwipeView /> : <ListasView />}
    </div>
  );
}
