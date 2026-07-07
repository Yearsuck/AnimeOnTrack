# Genre / watch stats dashboard

Part 3 of 3 of the watch-history feature. Depends on piece 1 (`2026-07-07-finished-anime-scraper-design.md`) for the `series_genres` table and `series.kind` column, and reads data that both piece 1 (swipe backfill) and piece 2 (swipe UI) write. No dependency on piece 2's UI directly — this screen would work even if piece 2 were skipped, as long as *something* populates `series_genres`.

## What counts

Any `series` row with `followed=1` counts toward the stats — not just rows created via the swipe flow. This includes series followed through the normal airing-catalog flow that the user has since finished watching. Consequence: series followed before this feature existed (or followed without ever touching "Descubrir") won't have `series_genres` rows yet, so they're invisible to the genre/type breakdowns until backfilled.

**Backfill for normal-flow followed series**: `refresh()` (`src-tauri/src/commands.rs`) gains one more step per followed series, alongside the existing one-cover-per-refresh fetch: if that series has no rows in `series_genres`, fetch its detail page once (`parse_series_detail`, same as piece 1's `Want`/`Seen` branches) and insert its genres + `kind`. Same politeness rule as the cover fetch it sits next to — one extra request per followed series per refresh cycle, never in bulk, and a failure here doesn't block the episode-scan step it runs alongside.

## Navigation

New top-level tab **"Stats"** in `App.tsx`'s `View` union and `.tabs` bar (5th tab, alongside Pendientes/En emisión/Biblioteca/Descubrir/Ajustes).

## Content

All numbers come from aggregate queries in `db.rs` — the frontend only renders pre-aggregated results, no client-side grouping.

- **Total watched**: count of `series` where `followed=1`.
- **Genre ranking**: horizontal bar list, `series_genres` grouped by `genre`, `COUNT(DISTINCT series_id)`, ordered descending. Only covers series that have genre data (see "What counts" above).
- **Type breakdown**: same shape, grouped by `series.kind` instead.
- **Total episodes watched**: `SUM(seen)` over `episodes` joined to followed series — already fully available, no new data needed.
- **Watch trend**: count of series "marked watched" per month, using `MIN(episodes.added_at)` per series as the proxy timestamp (the existing insertion-time column already used for `LibraryItem.last_added` — no new column). This is "when it was logged in the DB," not "when it was actually watched," which matters for historic swipe-backfilled entries (they'll cluster on whatever day the user swiped through their backlog) — acceptable given no real watch-date data exists to scrape.
- **Backlog size**: count of `series` where `backlog_status='want'`, shown as a secondary/contextual number, clearly separated from the "watched" numbers since it isn't watched.

## Backend

Three new query functions in `db.rs`:
- `get_genre_stats(source_id) -> Vec<(String, i64)>` — `(genre, count)`.
- `get_type_stats(source_id) -> Vec<(String, i64)>` — `(kind, count)`.
- `get_watch_trend(source_id) -> Vec<(String, i64)>` — `(year_month, count)`.

Plus the total-watched, total-episodes, and backlog-size counts, either as their own small queries or folded into one `get_watch_summary(source_id) -> WatchSummary` struct — implementation detail, whichever keeps `commands.rs` thin.

New Tauri commands wrapping these, mirroring the existing thin-wrapper pattern already used throughout `commands.rs`.

## Frontend

New `src/views/Stats.tsx`, one file per screen matching the existing convention (`AiringGrid.tsx`, `Library.tsx`, etc). Loads all stats on mount via the new API bindings in `src/api.ts`. No interactivity beyond display — no drill-down into "which series are in this genre" for v1 (could be a natural follow-up, not required now).

## Testing

- `db.rs` unit tests for each aggregate query against a small seeded DB (a couple of series with overlapping genres, confirm counts and ordering).
- `refresh()`'s genre-backfill step: test that a followed series without `series_genres` gets one populated, and that a series that already has rows isn't re-fetched.

## Explicitly out of scope here

- Per-genre drill-down (list of series within a genre from the stats screen).
- Any chart interactivity (filtering, date range picking) beyond the fixed set of aggregates above.
- Real watch-date tracking (would require the user to log dates, which no swipe/follow flow currently captures).
