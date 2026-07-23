# En emisión — "Esta temporada" filter (first episode < 3 months old)

**Date:** 2026-07-13
**Branch to implement on:** `feat/airing-this-season`
**Type:** feature. Backend (date parse + query) + frontend + i18n.
**Status:** approved (autonomous batch)

## Problem

Add to the En emisión tab a section/filter surfacing the **current-season** shows: airing series
whose **first** episode aired less than 3 months ago.

## Where the first-episode date comes from (verified — critical constraint)

- `episodes.released_at` is **Spanish free text**, format `"<mes> <día>, <año>"` e.g.
  `"junio 29, 2026"`, `"abril 1, 2026"`. Only **11 of 2195** episode rows are NULL/empty →
  reliably parseable with a Spanish month map. First episode = the **lowest `CAST(number AS INT)`**
  episode's `released_at`.
- **But episodes are scraped only for followed/opened series** (see [[project-scraping-scope]]).
  Live DB: `is_airing=1` series = **118**, of which only **37 have any episodes** (32 followed).
  The other 81 airing catalog cards have no episode rows and `next_episode_at IS NULL`.
- Ruled out as sources: `next_episode_at` is the *next* release (present only for on-listing
  series; a years-long show like One Piece has a soon `next_episode_at` too) → can't tell "started
  this season" from "ongoing for years". AniList `startDate` is unavailable — airing site rows are
  `anilist_id=NULL` (linking is on-demand, rarely done). Bulk-scraping the 81 to get first dates is
  forbidden by the scraping scope.

**Heuristic (with its documented limit):** "Esta temporada" = airing series that **have a scraped
first episode** whose parsed date is within the last 3 months. Series without episode data cannot
qualify (their start date is unknown) — this naturally scopes the section to shows the user follows
/ has opened, i.e. "the currently-airing shows I actually track that started this season". This is
the only honest answer the local data supports; documented in the UI hint.

## What exists (verified)

- `list_airing` (`commands.rs:585` → `db.rs:957`) returns `Vec<Series>` for `is_airing=1`, ordered
  by `next_episode_at`. Feeds `src/views/AiringGrid.tsx`.
- `chrono = "0.4"` is a backend dependency (usable for date math).
- Episode ordering elsewhere uses `CAST(number AS INTEGER) ASC` (`list_series_episodes`).

## Design

### Backend — Spanish date parse + first-episode-date query

- New pure module `src-tauri/src/dates.rs`: `parse_spanish_date(s: &str) -> Option<chrono::NaiveDate>`
  mapping `enero…diciembre` (case-insensitive; accept `setiembre` variant) from `"<mes> <día>,
  <año>"`. Unit tests over the real strings above + a null/garbage input → `None`.
- New `db.first_episode_dates(source_id) -> HashMap<i64 /*series_id*/, String>`: for each airing
  series, the `released_at` of its lowest-numbered episode
  (`SELECT series_id, released_at FROM episodes WHERE (series_id, CAST(number AS INT)) picks the
  min` — implement via a correlated subquery or `GROUP BY series_id` with `MIN(CAST(number AS INT))`
  join). Only rows with a non-null `released_at`.
- New command `list_airing_season(state) -> Vec<AiringItem>` where
  `AiringItem { series: Series, first_episode_at: Option<i64> }` (unix ts, from
  `parse_spanish_date` → `NaiveDate` → days). Compute `first_episode_at` = parsed first-episode
  date; `None` when no episodes or unparseable. Keeps `list_airing`'s ordering. (Alternatively add
  `first_episode_at` to the existing airing payload; a dedicated struct avoids disturbing the shared
  `Series` model and its many callers — prefer the struct.)
- The 3-month cutoff is computed **frontend-side** from `first_episode_at` so "now" is the user's
  clock and no re-query is needed when the day rolls over; backend just supplies the date.

### Frontend — filter in AiringGrid

- `AiringGrid.tsx` switches to `listAiringSeason()` (new `api.ts` wrapper). Add a segmented control
  **Todas | Esta temporada** (design-system `.tabs`/segmented, theme-aware).
- "Esta temporada" filters to items with `first_episode_at` set and `≥ now - 3 months`
  (`Date.now()/1000 - first_episode_at < ~92 days`, or compute 3 calendar months in JS). "Todas"
  shows everything (current behavior). Default **Todas** (don't hide the catalog by default).
- Show a small hint under the "Esta temporada" view explaining it only covers shows with known
  episode dates (followed/opened), so an empty-ish result is understood, not a bug.

### i18n (es.ts + en.ts)

- `airing.filterAll` = "Todas" / "All"
- `airing.filterSeason` = "Esta temporada" / "This season"
- `airing.seasonHint` = "Series seguidas que estrenaron hace menos de 3 meses" / "Followed series
  that premiered less than 3 months ago"

## Acceptance criteria

- `parse_spanish_date` unit tests pass for real strings incl. `"junio 29, 2026"`, `"abril 1, 2026"`;
  `None` for empty/garbage.
- `db.first_episode_dates` returns the lowest-numbered episode's date per airing series (unit test on
  a seeded DB with out-of-order episode inserts → picks episode "1").
- En emisión shows a Todas | Esta temporada control; "Esta temporada" lists only airing series whose
  first scraped episode is < 3 months old; "Todas" unchanged.
- `cargo test` green; `npx tsc --noEmit`; `npm run build` clean. No scraping added.

## Live verification (user)

Relaunch: En emisión → Esta temporada shows this-season follows; verify a show you started >3 months
ago is excluded and a brand-new one is included. Chrome harness previews the control markup.

## 2026-07 addendum: no longer effectively followed-only

The original heuristic above ("only shows have a scraped first episode, which in practice means
followed/opened ones") turned out to under-cover the feature — most airing series never got a
verdict at all, regardless of the user's intent. Fixed without touching the forbidden "no bulk
site scraping" constraint: the full AniList catalog (~22k titles) is *already* synced locally,
independent of site scraping or follow status (`anilist_catalog`, synced from the Catálogo tab).
`anilist_catalog` gained a nullable `start_date` column (same NULL-safe pattern as `status`/
`duration`/`studio`), synced from AniList's `Media.startDate`. `list_airing_season` now prefers
the scraped first-episode date when present (day-accurate, from a real episode), falling back to
`Db::catalog_start_dates_by_normalized_title` — a title match against the synced catalog — for
every other airing series, followed or not. Still `None` (no verdict) when the title isn't found
in the synced catalog or the Catálogo hasn't been synced since `start_date` was added — requires
a Catálogo resync to fully populate, same rollout story as every other additive AniList column.
