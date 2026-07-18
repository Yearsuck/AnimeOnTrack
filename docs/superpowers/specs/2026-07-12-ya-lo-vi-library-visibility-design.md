# "Ya lo vi" not shown/counted as watched in Library — root cause & fix

**Date:** 2026-07-12
**Branch to implement on:** `feat/ya-lo-vi-library`
**Type:** BUG (correctness). Root cause found with live DB evidence.
**Status:** approved (autonomous batch)

## Symptom

Anime marked "Ya lo vi" don't appear / aren't counted as watched in the Library.

## Two "Seen" paths (verified, `src-tauri/src/commands.rs`)

- **`decide_swipe` Seen** (site-sourced card, L1125): sets `followed=1` and inserts every
  scraped episode with `seen=true`. NO `watched_externally`. → shows in Library as a
  followed, fully-caught-up series. **Works.**
- **`decide_catalog_card` Seen** (AniList catalog card, L1827): sets
  `watched_externally=1, followed=0, backlog_status=NULL`, and inserts NO episodes (AniList
  is metadata-only). This is the "I watched it outside the app" marker.

## Root cause

`db.list_library` (`src-tauri/src/db.rs` L842) filters `WHERE s.source_id=?1 AND s.followed=1`.
Catalog "Ya lo vi" rows have `followed=0`, so they are **excluded from the Library entirely**.

Live DB evidence: **60** rows have `watched_externally=1 AND followed=0` — all invisible in
Library. (5 more have both `followed=1 AND watched_externally=1` and are already visible.)

## Fix

### Backend — include watched-externally rows in the Library

1. `db.list_library` (db.rs:842): change the filter to
   `WHERE s.source_id=?1 AND (s.followed=1 OR s.watched_externally=1)`.
   The existing `GROUP BY s.id` already dedups the "both" rows. Add `s.watched_externally` to
   the SELECT list and read it out.
2. `models::LibraryItem` (models.rs:40): add `#[serde(default)] pub watched_externally: bool`.
   (Add to `LibraryItem`, NOT to `Series` — `Series`'s SELECT column lists are spread across
   many queries; keep the blast radius here.)
3. Populate it in `list_library`'s row mapping.

### Frontend — classify & display

4. `src/types.ts`: add `watched_externally: boolean` to the `LibraryItem` type.
5. `src/views/Library.tsx` `statusOf` (L18): add as the FIRST check
   `if (it.watched_externally) return "completed";`. These land in the **Completadas** section
   (already `showAction={false}`; `next_episode` is `None` so no play button renders).
6. Card display for a watched-externally item (0 episodes): the current `{seen} / {total}` reads
   "0 / 0" with a 0% bar. Replace that, **only when `watched_externally && total_episodes===0`**,
   with a "✓ Vista fuera de la app" / "✓ Watched elsewhere" label. New i18n key
   `library.watchedExternally` in BOTH `src/i18n/catalog/es.ts` and `en.ts` (missing key = tsc error).
   Leave the progress bar/text unchanged for normal followed series.

### Nice-to-have (implement if cheap, else note as skipped)

7. Allow reclassify (⋯ menu) on watched-externally Completed cards so the user can undo
   ("Marcar como no vista" → `reclassify_series(id, "None")`). The Completed section currently
   passes `showMenu={false}`; enabling a menu only for watched-externally cards is optional.

## Acceptance criteria (verifiable without live UI)

1. `cargo test --manifest-path src-tauri/Cargo.toml` green.
2. New `db.rs` unit test: seed one `followed=1` series with episodes and one
   `watched_externally=1, followed=0` series with zero episodes; assert `list_library` returns
   BOTH, and the watched-externally item has `watched_externally == true`, `total_episodes == 0`.
3. `npx tsc --noEmit` clean (proves the new type field + i18n key wired).
4. `npm run build` OK.
5. `library.watchedExternally` present in both catalogs.

## Verify live (NOT tool-reachable — state honestly)

Relaunch; the ~60 "Ya lo vi" catalog titles now appear under Completadas with the "Vista fuera
de la app" label. Cannot be screenshot-verified here.

## Overlap note (do NOT implement here)

"cuentan correctamente" (Stats counting of watched series) is **Task 8** (`get_watch_summary`).
This spec fixes Library visibility + classification only. Task 8's spec must make its counts
count `watched_externally` consistently — flagged there.

## Out of scope

Stats metrics (Task 8). The `decide_swipe` Seen path (already works). No change to how "Seen"
is recorded — only how the Library reads it.
