# Stats clarity — metrics & labels

**Date:** 2026-07-12
**Branch to implement on:** `feat/stats-clarity`
**Type:** clarity / small feature. Backend + frontend + i18n.
**Status:** approved (autonomous batch)

## Problem

The Estadísticas summary tiles are unclear:
- Episodes watched is fine.
- **Missing:** a count of *animes* (distinct series), collapsing seasons of the same show into
  one (3 seasons of X = 1 anime, even though they're 3 rows).
- "Series seguidas" is ambiguous (only airing? all?).
- "Pendientes en backlog" is confusing → should read like "Series pendientes".

## What exists (verified)

- `get_watch_summary` (commands.rs:570 → db.rs:763) returns `WatchSummary { followed_series,
  episodes_watched, episodes_total, backlog_want }` — `followed_series` = COUNT of `followed=1`
  (all, airing or not); `backlog_want` = COUNT `backlog_status='want'`; episodes over followed.
- `models::WatchSummary` (models.rs:103). `Stats.tsx` tiles (L90-112) use i18n keys
  `stats.episodesWatched`, `stats.followedSeries`, `stats.backlogPending`.

## Design

### Franchise (season-collapsing) count

Add a pure helper `franchise_key(title: &str) -> String` (in a small module or `db.rs`) that
strips season/part markers so seasons collapse:
- normalize (reuse `matching::normalize_title` — accents/case/punct/noise stripped),
- then strip trailing season/part markers: `temporada N`, `season N`, `part N`, `parte N`,
  `cour N`, `Nth season`, a trailing standalone integer, and trailing Roman numerals
  (`ii`, `iii`, `iv`, `v`, … up to a small set), and `final season`.
- Collapse resulting empty → fall back to the normalized title (never key to "").
Distinct-anime count = number of distinct `franchise_key` among the counted set.

**Counted set:** to stay consistent with Task 3's "visto" semantics, count distinct franchises
over series the user is tracking: `followed=1 OR watched_externally=1`. Document this in the tile
help. (Episodes stay over `followed=1` as before — episodes only exist for followed/site rows.)

### New `WatchSummary` fields + query

Extend `WatchSummary` with `distinct_anime: i64` and (for the relabel) keep `followed_series`.
Compute `distinct_anime` in `db.get_watch_summary`: select titles of
`followed=1 OR watched_externally=1` rows, map through `franchise_key` in Rust, count distinct.

### Tiles / labels (frontend + i18n, both catalogs)

`Stats.tsx` tiles become:
1. **Episodios vistos** — `episodes_watched / episodes_total` (unchanged key
   `stats.episodesWatched`).
2. **Animes** (new tile) — `distinct_anime`, with a small helped subtitle "temporadas contadas
   como una". New key `stats.distinctAnime` + `stats.distinctAnimeHelp`.
3. **Series en seguimiento** — `followed_series`, relabel `stats.followedSeries` to clarify it's
   every series you're actively following (airing or not). Update the es/en strings (key name
   stays; only the text changes).
4. **Series pendientes** — `backlog_want`; relabel `stats.backlogPending` text to "Series
   pendientes" / "Pending series".

Show 4 tiles in the existing `.grid`.

## Acceptance criteria (verifiable without live UI)

1. `cargo test` green with new unit tests:
   - `franchise_key`: "Tensei shitara Slime Datta Ken Temporada 4" and "Tensei shitara Slime
     Datta Ken" → same key; "Overlord IV" and "Overlord" → same; two genuinely different shows →
     different keys; a title that is *only* a season marker never keys to empty.
   - `get_watch_summary` (db test): distinct_anime collapses multiple season rows of one show to
     1 and counts a watched_externally-only row.
2. `npx tsc --noEmit` clean (new field on the `WatchSummary` type in `types.ts`).
3. `npm run build` OK.
4. New i18n keys in BOTH catalogs; changed label texts updated in both.

## Verify live (NOT tool-reachable — state honestly)

Relaunch, open Estadísticas, confirm the Animes count is sensible (seasons collapsed) and the
labels read clearly. Cannot be screenshot-verified here.

## Out of scope

Task 9 (circles↔bars) and Task 10 (graph). Genre/type stat lists unchanged.
