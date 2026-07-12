# Cross-site follow & progress carry-over — design

**Date:** 2026-07-12
**Branch to implement on:** `feat/cross-site-carryover`
**Type:** correctness / data-continuity. **Riskiest task — keep title matching conservative.**
**Status:** approved (autonomous batch)

## Problem

Switching the active site in Settings makes the series you followed "disappear": your follows
and watch progress are still in the DB but tagged with the *old* site's `source_id`, and every
library/airing/pending query filters on the active `source_id`. Because site titles don't match
1:1 between sites, the new site starts with none of your follows.

## Real model (verified, `src-tauri/src/db.rs`)

- `series`: `UNIQUE(source_id, slug)`, columns incl. `followed`, `is_airing`. Each site is its
  own `sources` row / `source_id`; each has its own `series` + `episodes` rows. No cross-site
  identity exists.
- `episodes`: `UNIQUE(series_id, url)`, `seen`, `seen_at`, `number` (TEXT).
- `set_seen_cascade(series_id, number, seen)` marks every episode ≤ `number` seen.
- `matching::best_match(queries, candidates) -> Option<MatchResult{index, score}>`,
  `MATCH_THRESHOLD = 0.72` (tuned; decoys score below, true matches above). Now also
  `matching::normalize_title` (public, from Task 4).
- Switching: `set_active_site`/`switch_site_core` (commands.rs:499) only flips the active
  `source_id`; **nothing is deleted**. `scan_airing_via_mirrors` (commands.rs:354) upserts the
  new site's airing series on scan.

## Design — carry the follow + a progress watermark on switch, matched by title

No merge, no delete, no new duplicate rows, no burst of scrapes. The follow is carried onto the
new site's *own* series row; progress is carried as a watermark applied lazily during the normal
refresh episode-fetch.

### Schema

Add nullable `series.carried_seen_number INTEGER` via the existing `ensure_column` migration
pattern (db.rs ~L217). It records "when this series' episodes are next fetched, mark everything
up to this episode number seen, once."

### New DB helpers

1. `followed_titles_with_watermark(exclude_source_id) -> Vec<(String /*title*/, i64 /*watermark*/)>`
   — for every `followed=1` series on **other** sources, its title and
   `COALESCE(MAX(CASE WHEN e.seen=1 THEN CAST(e.number AS INTEGER) END), 0)` (highest *seen*
   episode number; 0 if none seen). Dedup by keeping the max watermark per normalized title if
   two other-source rows collide.
2. `carry_follow(series_id, watermark)` — `UPDATE series SET followed=1, carried_seen_number=?2
   WHERE id=?1 AND followed=0` (only carries onto a not-yet-followed row).
3. `take_carried_seen_number(series_id) -> Option<i64>` — read then `UPDATE ... SET
   carried_seen_number=NULL` (apply-once), or fold into the refresh flow.

### Carry-over on switch (Phase 1) — pure match, testable

Factor the matching into a pure fn
`plan_carryover(new_site_series: &[Series], followed_elsewhere: &[(String, i64)]) ->
Vec<(usize /*index into new_site_series*/, i64 /*watermark*/)>`:
for each new-site series NOT already followed, `best_match([its title], followed_elsewhere titles)`;
on a match ≥ `MATCH_THRESHOLD`, record (index, that entry's watermark). Take the single best
match per new-site series. Unit-testable with no DB.

Call it inside `scan_airing_via_mirrors` (or a dedicated `carry_over_follows` step invoked right
after it, within the switch flow) AFTER the new site's series are upserted: load
`followed_titles_with_watermark(new_source_id)`, run `plan_carryover`, and `carry_follow(...)`
each hit. This runs on the airing-scan (allowed scrape context) and touches **no** extra network.

### Progress applied lazily (Phase 2)

In `refresh()`'s per-series insert block (commands.rs ~L852-869), after inserting a followed
series' episodes: if `carried_seen_number` is `Some(n)`, call `set_seen_cascade(series_id, n,
true)` and clear the column (apply-once). This bounds work to the normal refresh — no scrape
burst on switch. The switch flow already triggers a scan; the app's on-open/refresh path then
fetches the carried series' episodes and applies the watermark.

## Guards / correctness

- Only carry onto `followed=0` new-site rows (never override an existing follow).
- One best match per new-site series; `MATCH_THRESHOLD` guards false links (a wrong carry would
  falsely follow + mark-seen — conservative threshold is essential; do NOT lower it here).
- Old-site rows keep their follow (invisible under the new site, restored on switching back). No
  double-count in views (all per active `source_id`).
- `carried_seen_number` cleared after application so a later real refresh never re-forces seen.

## Acceptance criteria (verifiable without live UI)

1. `cargo test --manifest-path src-tauri/Cargo.toml` green.
2. Pure `plan_carryover` tests: exact/normalized title match carries with the right watermark; a
   decoy (score < threshold) does NOT carry; an already-followed new-site series is skipped;
   best-of-several chosen.
3. DB tests: `followed_titles_with_watermark` excludes the active source and computes the
   seen-watermark correctly (incl. 0 when nothing seen, ignoring non-numeric/recap rows via CAST);
   `carry_follow` sets followed + column only on `followed=0` rows; a refresh-style application of
   `carried_seen_number` marks ≤n seen via cascade and clears the column.
4. No duplicate rows created (carry updates the existing new-site row).
5. `npx tsc --noEmit` + `npm run build`.

## Verify live (NOT tool-reachable — state honestly)

Follow some series on site A, switch to site B in Settings, confirm the same shows come across as
followed with progress after the ensuing refresh, and switching back to A is unchanged. Cannot be
screenshot-verified here.

## Scope guard

Honors `project-scraping-scope`: matching is local (no per-title scrape); progress applies during
the already-happening airing/refresh fetches, not a new sweep. Respects `SCRAPE_PERMITS`. Keep
the `adapter`/`scraper_engine` untouched.

## Out of scope

A true global cross-site identity refactor (canonical id table). Carrying `want`/`discarded`
classifications across sites (only active follows + progress here). UI beyond what a normal scan
already shows.
