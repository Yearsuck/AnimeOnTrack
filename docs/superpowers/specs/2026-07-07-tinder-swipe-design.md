# Tinder-style swipe mode ("Descubrir")

Part 2 of 3 of the watch-history feature. Depends on piece 1 (`2026-07-07-finished-anime-scraper-design.md`) for `discover_swipe_card`, `decide_swipe`, `start_watching` and the `series_genres`/`backlog_status` data model — this piece is the UI consuming those commands, plus two small backend additions (undo, discarded-list actions) it needs that piece 1 didn't cover. Piece 3 (genre stats) reads the data this piece writes but has no dependency the other way.

## Navigation

New top-level tab **"Descubrir"** in `App.tsx`'s `View` union and `.tabs` bar, alongside `pending`/`airing`/`library`/`settings`. Owns two internal sub-views (a small in-component tab strip, not part of the global nav):

- **Swipe** (default) — one card at a time.
- **Listas** — two sections, "Quiero ver" and "Descartados".

## Swipe sub-view

**Card content** — only what the listing scrape already returned for that card (poster, title, type badge, the single genre it was found under). No detail-page fetch happens before a decision, per piece 1's design (that fetch is what makes the decision cost 1 request, not showing-the-card cost 1 request).

**Actions**: three buttons — **✕ Descartar**, **★ Quiero ver**, **✓ Ya lo vi** — each calling `decide_swipe(card.url, decision)`, then requesting the next card via `discover_swipe_card`. Optimistic UI: card animates out immediately on click, next card (already prefetched into the local buffer per piece 1) swaps in without a loading flash in the common case; only shows a loading state when the local buffer is empty and a fresh page scrape is in flight.

**Undo**: a 4th button, **↺ Deshacer**, enabled only right after a decision. New backend command `undo_last_swipe(state) -> Result<(), String>`: `AppState` gains an in-memory `Mutex<Option<i64>>` (`last_swiped_series_id`), set after every successful `decide_swipe`. Undo hard-deletes that series row (cascading to its `series_genres` and any inserted episodes) and clears the slot; calling undo twice in a row (nothing to undo) is a no-op, not an error. This is intentionally session-only — no persistence, no multi-level history — since it's a "fix my last misclick" safety net, not an audit log.

**Keyboard shortcuts** (swipe sub-view only, not global): `←` = descartar, `→` = quiero ver, `↑` = ya lo vi, `Ctrl+Z` = deshacer.

**Empty/error states**: if `discover_swipe_card` returns `None` (buffer + fresh page both fully filtered), immediately request again — this is a normal "everyone on that page was already decided" case, not user-visible, but cap silent retries (e.g. 5) before showing a "no se han encontrado más animes por ahora, prueba más tarde" empty state, in case genre coverage is exhausted. Scrape failures (mirror down, empty parse) follow the same pattern already used elsewhere in the app: skip and retry, never a blocking modal.

## Listas sub-view

**"Quiero ver" section**: rows show poster, title, and full genre list (already fetched at swipe time, stored in `series_genres`). Two actions per row:
- **Empezar a ver** → calls piece 1's `start_watching(series_id)`. On success, removed from this list (now `followed=true`, shows up in the existing Library view via its normal query — no changes needed there since Library already lists all `followed` series).
- **Descartar** → sets `backlog_status='discarded'` (reuse a small new `set_backlog_status(series_id, status)` command rather than routing back through `decide_swipe`, which expects a not-yet-in-DB card).

**"Descartados" section**: rows show poster + title only (genres/synopsis were never fetched for pure discards). Two actions:
- **Mover a quiero ver** → if `series_genres` is empty for this row (the common case, since discard never fetches detail), fetch the detail page now (same path `decide_swipe`'s `Want` branch uses) before setting `backlog_status='want'`; if already populated (e.g. it was previously a "want" that got discarded), just flip the status. Either way, one new command, e.g. `promote_discarded(series_id)`.
- **Eliminar del todo** → hard-delete the series row (`delete_series(series_id)`, new command). Safe unconditionally here: discarded rows never have episodes.

Both list sections load via a new query, e.g. `list_backlog(status: 'want' | 'discarded') -> Vec<Series>` (or one command returning both, frontend splits by `backlog_status` — implementation detail, either is fine).

## Error handling

Consistent with the rest of the app: no blocking modals. Failed scrapes during a decision (e.g. detail-page fetch fails after the user already clicked "quiero ver") should not lose the user's intent — retry the fetch once via the existing mirror-fallback path; if it still fails, fall back to inserting the series row with whatever data is already in hand (title/poster/single genre from the card) rather than dropping the decision entirely, and leave it eligible for a later genre-list backfill (out of scope to build that backfill now — just don't make the failure mode "the swipe silently did nothing").

## Testing

- Frontend: manual verification via the `run` skill once implemented (per-project convention — this is a Tauri desktop app, no existing component-test harness to extend).
- Backend: unit tests for `undo_last_swipe` (deletes cascade correctly, no-op when nothing to undo), `set_backlog_status`, `promote_discarded` (both the "needs detail fetch" and "already has genres" branches), `delete_series`.

## Explicitly out of scope here

- Genre stats screen — piece 3.
- Multi-level undo history.
- Filtering the swipe deck by genre (deck is fully random across all genres, per the approved piece 1 design).
