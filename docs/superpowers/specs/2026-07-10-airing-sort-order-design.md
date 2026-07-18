# "En emisión": default sort newest-first

## Problem

`db.rs::list_airing` is `... WHERE is_airing=1 ORDER BY title`, and `AiringGrid.tsx` renders that order verbatim. Alphabetical is useless for a schedule: the show that dropped an episode an hour ago sits between two that aired six days ago.

## What date field actually exists

`series` has **no date column at all** (`id, source_id, slug, title, url, cover_url, is_airing, followed` + the later `backlog_status`, `kind`, `anilist_id`, `watched_externally`). `episodes.released_at` exists but only for **followed** series — episode lists are never scraped for the rest of the airing grid, so it can order at most a fifth of the page and would leave everything else NULL. Sorting the airing grid by it is not viable.

The real signal is on the airing cards themselves, and the adapter currently discards it. From the real `airing.html` fixture:

```html
<span class="epx cndwn" data-cndwn="8859" data-rlsdt="1783350140">0d 2h 27m</span>
```

`data-rlsdt` is the **Unix timestamp of the next episode's release** (fixture values decode to 2026-07-06T15:02Z / 18:44Z and 2026-07-07T14:20Z, all shortly after the capture date).

## Dependency

This spec **requires** `2026-07-10-scraper-performance-design.md`, which introduces `series.next_episode_at` (from `data-rlsdt`) and `series.site_episode_count` (from `.sb`), parsed in `parse_airing` and written by `upsert_series`. Implement that spec first, or implement the two together. Do not add a duplicate column.

## Sort criterion, and why

Order by `next_episode_at` **descending**, NULLs last.

For a weekly series, the next release is roughly `last release + 7 days`. So the series whose episode dropped most recently has the *furthest-away* next episode, and the one about to air next has the *oldest* last episode. Descending `next_episode_at` therefore reads as **"most recently released first"** — exactly the requested newest→oldest ordering — while being derived from the only date the site actually gives us for every card.

This is an inference, not a fact the site states. Two consequences the implementer must handle:

- **Non-weekly series break the assumption** (dailies, irregular ONA drops). Accept it: the ordering is right for the overwhelmingly weekly common case, and no better per-card signal exists without scraping ~150 series pages, which the politeness constraints forbid.
- **`data-rlsdt` may be absent or stale** (a card with no countdown; a series whose next episode already aired and whose card hasn't rolled over). `NULL` sorts last. Tie-break by `title` so the order is stable across calls — without a secondary key, equal/NULL timestamps reorder run to run.

Verify on live HTML what a card looks like for a series whose next episode has just aired, and write down what you saw. If `data-rlsdt` turns out to sit in the past for those, they will bubble to the bottom rather than the top, which is **wrong** for this feature — in that case, sort by `COALESCE(next_episode_at, 0)` descending still misplaces them, so fall back to ordering on `next_episode_at - 7 days` normalized into "last release" and say so explicitly in the summary.

## Design

- `list_airing(source_id)` → `ORDER BY next_episode_at DESC NULLS LAST, title` (SQLite: `ORDER BY next_episode_at IS NULL, next_episode_at DESC, title`). Add `CREATE INDEX IF NOT EXISTS idx_series_next_ep ON series(source_id, is_airing, next_episode_at)` in `Db::init`.
- `AiringGrid.tsx`: no sort UI. The backend order is the order. (A sort dropdown is a separate feature; the request is a better default, not a control.)
- Optional, cheap, and worth it: show the countdown on each card — a small `.chip`-styled label ("en 2 h", "hace 3 h") computed client-side from `next_episode_at`, using the existing design-system chip. It makes the ordering legible instead of mysterious. Recompute on render only; no timer.

## Acceptance criteria (verifiable)

1. `cargo test` passes with a new `db.rs` test: seed four series — two with distinct `next_episode_at`, one `NULL`, one sharing a timestamp with another — and assert the returned order is newest-first, NULL last, alphabetical tie-break.
2. `npx tsc --noEmit`, `npm run build` pass.
3. Live: "En emisión" no longer alphabetical; the top cards are the series whose episodes released most recently. Cross-check at least three cards against the site's own schedule page ordering and against `SELECT title, next_episode_at, datetime(next_episode_at,'unixepoch') FROM series WHERE is_airing=1 ORDER BY next_episode_at DESC LIMIT 5`.

## Live verification required

- Screenshot of the reordered grid.
- The `sqlite3` query above, showing real timestamps that agree with the on-screen order.
- A sentence stating what a just-aired series' card actually looked like on the live site (the open question above), and whether the fallback was needed.

## Explicitly out of scope

- A user-facing sort selector.
- Sorting the airing grid by user progress or by `episodes.released_at`.
- Backfilling `next_episode_at` for series no longer on the airing list.
