# Descubrir deck must exclude already-followed/watching anime — root cause & fix

**Date:** 2026-07-12
**Branch to implement on:** `feat/discover-exclude-followed`
**Type:** BUG (correctness) + prerequisite for Task 5 (recommendation engine).
**Status:** approved (autonomous batch)

## Symptom

The Descubrir deck offers anime the user already follows / is watching (and ones they've
marked watched-externally). Pointless to re-offer decided titles.

## Root cause (live DB evidence)

Deck: `discover_catalog_card` (`commands.rs` L1733) → `db.random_catalog_anime_in_genre`
(`db.rs` L1399). Its only exclusion (db.rs L1422):
```sql
AND c.id NOT IN (SELECT anilist_id FROM series WHERE anilist_id IS NOT NULL)
```
This excludes only catalog anime whose AniList id already exists on a `series` row.

**But ALL 132 followed series have `anilist_id = NULL`** (verified live: `followed=1 AND
anilist_id IS NOT NULL` → 0). Site rows (airing scan / site swipe) never get an `anilist_id`
— linking is on-demand and rarely happens. So the exclusion catches **~zero** engaged series.
A title-collision query confirms the deck re-offers followed shows: "Black Clover", "GOBLIN
SLAYER II", "Overlord IV", "EDENS ZERO", … all exist as `followed=1, anilist_id=NULL` rows
AND as catalog entries. Also `want` (16) and `discarded` (110) site rows lack anilist_id.

## Fix — normalized-title exclusion (reuse `matching.rs`)

Site series store romaji-ish titles that match catalog `title`/`title_romaji`/`title_english`
after normalization. `src-tauri/src/matching.rs` already has the exact normalizer needed
(`normalize`: strip accents, lowercase, collapse punctuation, strip noise suffixes) — currently
private.

1. **Expose the normalizer:** make `matching::normalize` public as
   `pub fn normalize_title(s: &str) -> String` (rename or thin pub wrapper; keep internal callers
   working). Add a unit test pinning it (`normalize_title("Overlord IV") == normalize_title` of
   the same string; accent/case/punct cases already covered by existing `normalize` tests).

2. **Build the engaged-title set** (in `discover_catalog_card`, once per call, passed down):
   new `db.engaged_series_titles(source_id) -> Vec<String>`:
   ```sql
   SELECT title FROM series
   WHERE source_id=?1
     AND (followed=1 OR watched_externally=1 OR backlog_status IN ('want','discarded'))
   ```
   Normalize each into a `HashSet<String>`.

3. **Exclude by normalized title in the pick.** `random_catalog_anime_in_genre` currently does
   `ORDER BY RANDOM() LIMIT 1`. Change it to accept the engaged set and fetch a batch
   (`ORDER BY RANDOM() LIMIT 40`), then in Rust return the first candidate whose normalized
   title variants (`normalize_title(title)`, `..(title_romaji)`, `..(title_english)`, skipping
   NULLs) do **not** intersect the engaged set. Keep the existing `anilist_id NOT IN` clause and
   the quality floor / format ban logic. If the whole batch is excluded, return `Ok(None)` for
   that genre (the orchestrator's `pool.remove` already advances to the next genre).
   - Signature becomes `random_catalog_anime_in_genre(&self, genre, banned_formats, excluded_norm_titles: &HashSet<String>)`.
   - Batch size 40 is a safe margin: even a heavily-followed genre rarely has >40 of the
     user's exact titles in one RANDOM draw; if it does, the genre falls through, which is fine.

4. Scope everything to the active `source_id` (consistent with the per-source model).

## Why not fuzzy `best_match` / anilist backfill

- Fuzzy matching every batch candidate against 130+ engaged titles risks **false exclusions**
  (dropping legitimately different shows) and is heavier. Normalized-exact across the 3 title
  variants already catches the demonstrated collisions (site titles are romaji-ish).
- Backfilling `anilist_id` on all followed site series via title match is a larger, riskier
  change (wrong links) and out of scope here; the exclusion-set approach is safe and reversible.

## Acceptance criteria (verifiable without live UI)

1. `cargo test --manifest-path src-tauri/Cargo.toml` green.
2. New `db.rs` unit test: seed a catalog entry titled "Overlord IV" (with a genre) and a
   `series` row `followed=1, anilist_id=NULL, title='Overlord IV'`; assert
   `random_catalog_anime_in_genre` for that genre never returns the "Overlord IV" catalog entry
   (excluded by normalized title despite the NULL anilist_id). Add a control where an
   un-followed catalog title IS returnable.
3. New/updated `matching.rs` test for the public `normalize_title`.
4. `npx tsc --noEmit` + `npm run build` (no frontend change expected, but run them).

## Verify live (NOT tool-reachable — state honestly)

Relaunch, swipe the Descubrir deck for a while, confirm no already-followed/watching title is
offered. Cannot be screenshot-verified here.

## Interface note for Task 5

Task 5 (recommendation engine) will replace the pure-RANDOM within-genre pick with a scored
pick over this same batch, reusing the engaged-title exclusion set and batch plumbing this task
introduces. Keep `random_catalog_anime_in_genre`'s batch fetch factored so Task 5 can score the
batch instead of taking the first survivor.

## Out of scope

Recommendation scoring (Task 5). Catalog *browse* view exclusion (the browse deliberately shows
everything; only the swipe deck is in scope). No scraping (all local).
