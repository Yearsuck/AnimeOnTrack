# Stats — stale refresh bug + metric concept fix

**Date:** 2026-07-13
**Branch to implement on:** `feat/stats-stale-refresh-concepts`
**Type:** BUG (stale) + clarity (concepts). Backend + frontend + i18n.
**Status:** approved (autonomous batch)

## Problem

1. **Stale bug:** the Estadísticas screen frequently shows out-of-date numbers. Following a
   series, marking episodes seen, reclassifying (want/discarded/ya-vi), or a cross-site follow
   do not update the tiles/charts/graph until an *airing refresh* runs or the app is relaunched.
2. **Concept mislabel:** the tiles "Series en seguimiento" and "Series pendientes" are
   conceptually wrong. Verified against the live DB (active source = animeytx, `source_id=1`):
   - "Series en seguimiento" = `COUNT(followed=1)` = **133**, of which **99 are finished**
     (`is_airing=0`) and only **34 are airing**. It mixes "airing shows I follow" with an
     archive of finished shows.
   - "Series pendientes" = `COUNT(backlog_status='want')` = **174**. That is the *Quiero ver*
     wishlist from Descubrir, NOT episodes pending to watch. The count of series I actually
     follow with ≥1 unseen episode is **15**. Users read "pendientes" as "things I still have
     to watch" → the number (174) is meaningless to them.

## What exists (verified)

- `Stats.tsx` (`src/views/Stats.tsx`): component stays mounted forever (App.tsx hides with CSS,
  see `2026-07-10-stats-graph-cache-design.md`). `load()` runs `getWatchSummary/getGenreStats/
  getTypeStats/getStatsGraph` in parallel. It is called: once on mount (L40-42); on the
  `refresh-progress` event **only when `current >= total`** (L49-62) — reloading live if
  `active`, else setting `dirtyRef`; and on the active-transition effect (L65-71) **only if
  `dirtyRef` is set**; and after genre backfill.
- **`refresh-progress` is emitted only by the airing scan** (`commands.rs` `refresh`). None of
  `set_followed`, `start_watching`, `set_seen_cascade`, `reclassify_series`,
  `set_watched_externally`, cross-site carry emit it → Stats never learns those happened.
  Because the component never remounts and the active-transition reload is gated on `dirtyRef`
  (which only `refresh-progress`/backfill set), switching to the Stats tab after any of those
  mutations shows stale data.
- `db.get_watch_summary(source_id)` (`db.rs:866`) returns `WatchSummary { followed_series,
  distinct_anime, episodes_watched, episodes_total, backlog_want }`:
  - `followed_series` = `COUNT(*) series WHERE source_id=? AND followed=1`.
  - `backlog_want` = `COUNT(*) WHERE backlog_status='want'`.
  - `distinct_anime` = distinct `franchise_key` over `followed=1 OR watched_externally=1`.
- `models::WatchSummary` (`models.rs:110`), `#[serde(default)]` on `distinct_anime`.
- i18n keys (`src/i18n/catalog/es.ts` L220-224, mirrored in `en.ts`):
  `stats.episodesWatched`, `stats.distinctAnime` (+Help), `stats.followedSeries`,
  `stats.backlogPending`.
- `series.is_airing INTEGER DEFAULT 1` column exists (confirmed via `PRAGMA table_info`).

## Design

### Fix 1 — stale refresh (reload on activation)

Keeping the component mounted preserves three.js/d3-force state; it does **not** require avoiding
data refetches (refetching `seriesList` doesn't tear down the graph — `StatsGraph` already
diff-guards layout via its `structuralKey`). So: **reload whenever the tab becomes active**,
regardless of `dirtyRef`.

Change the active-transition effect (L65-71) to call `load()` on every false→true transition:

```ts
useEffect(() => {
  if (active && !wasActiveRef.current) {
    dirtyRef.current = false;
    load();
  }
  wasActiveRef.current = active;
}, [active]);
```

Keep the mount-time `load()` and the `refresh-progress` live-reload (so a scan finishing while
Stats is already open still updates). Drop the `dirtyRef` gate on activation. `dirtyRef` may be
removed entirely or left as a no-op; prefer removing it for clarity. This guarantees any mutation
made on another tab is reflected the next time the user opens Estadísticas — the only moment the
numbers are actually looked at. No new Rust events, no polling. This reload also feeds fresh
`seriesList` to `StatsGraph`, which is a prerequisite for T6a (graph picking up new follows).

### Fix 2 — metric concepts

Redefine the two ambiguous tiles precisely and add the wishlist as its own clearly-named tile.
New `WatchSummary` fields (extend, keep existing for compatibility):

- `airing_followed` = `COUNT(*) series WHERE source_id=? AND followed=1 AND is_airing=1` → the
  shows I follow that are still airing (**34**). Replaces the meaning of the "en seguimiento" tile.
- `pending_to_watch` = number of **followed** series with at least one unseen episode:
  `SELECT COUNT(DISTINCT s.id) FROM series s JOIN episodes e ON e.series_id=s.id
   WHERE s.source_id=? AND s.followed=1 AND e.seen=0` (**15**). This is the real "pendientes de
   ver" backlog. Replaces the meaning of the "pendientes" tile.
- Keep `backlog_want` (**174**) but surface it under an unambiguous label ("En «Quiero ver»").
- Keep `followed_series` in the struct (still used by graph/other callers) but it stops driving a
  tile labelled "en seguimiento".

Tiles in `Stats.tsx` become five (the `.grid` flex/grid already wraps): Episodios vistos, Animes,
**Siguiendo en emisión** (`airing_followed`), **Pendientes de ver** (`pending_to_watch`),
**En «Quiero ver»** (`backlog_want`).

### i18n

- Relabel `stats.followedSeries` → "Siguiendo en emisión" / "Currently airing (followed)".
- Relabel `stats.backlogPending` → "Pendientes de ver" / "Pending to watch", and add a help line
  `stats.backlogPendingHelp` = "series seguidas con episodios sin ver" / "followed series with
  unseen episodes".
- Add `stats.wishlist` = "En «Quiero ver»" / "On «Want to watch»" (+ optional
  `stats.wishlistHelp`).
- Every new key in BOTH `es.ts` and `en.ts` (missing key → `tsc` fails via `Record<keyof typeof
  es, string>`).

## Acceptance criteria

- After following/unfollowing a series (from AiringGrid or SeriesDetail), then opening Estadísticas
  **without** running an airing refresh, the tiles reflect the change. Same for marking episodes
  seen and for reclassify (want/discarded/ya-vi).
- "Siguiendo en emisión" shows the count of `followed=1 AND is_airing=1` (34 on current DB), not
  133.
- "Pendientes de ver" shows followed series with unseen episodes (15 on current DB), not the 174
  wishlist.
- A tile clearly labelled "En «Quiero ver»" shows 174.
- `cargo test` green (add a `get_watch_summary` unit test asserting the new fields on a seeded DB:
  one followed+airing+unseen, one followed+finished+all-seen, one want-only → airing_followed=1,
  pending_to_watch=1, backlog_want=1). `npx tsc --noEmit` and `npm run build` clean.

## Live verification (what the user must eyeball)

Relaunch the app: follow a title, switch to Estadísticas, confirm the numbers moved without a
scan; confirm the three new labels read unambiguously. Not tool-reachable in the Tauri WebView2
window; harness only covers markup, not the mutation flow.
