# Plan 2026-10-30: data and stats accuracy, fewer estimates

Source: three read-only agy reports (`.worktrees/agy-audit-{stats-backend,app-data,stats-front}/.agy-report-*.md`).
Every item below was re-checked by the supervisor at the cited line. Claims REJECTED after checking are listed at the end.

## A. Stats backend bugs (PR 1, `fix/stats-accuracy`) — `src-tauri/src/db/stats.rs`
TDD: one failing test per item first.
1. `pending_to_watch` counts "Ya lo vi" series (`stats.rs:1573`, no `watched_externally=0`): Stats says "1 pendiente",
   Pendientes is empty. Add the filter (same rule as `list_pending`, `airing.rs:427`).
2. Dusty watchlist lists finished series (`stats.rs:2087`, no unseen-episode condition): keep only groups with unseen
   episodes left.
3. Dusty groups by `canon_key` (`stats.rs:2121`), so S1 is "dusty" while S2 is being watched: group by `franchise_key`.
4. Cross-site real progress takes the best single site (`RollupBuilder::finish`, `stats.rs:593`): S1 seen on site A
   + S2 on site B = 12, not 24, while the heatmap counts 24. Count distinct canonical episodes per franchise
   (same dedupe the activity charts already use, `deduplicated_seen_episodes`, `stats.rs:1616`), so summary and charts agree
   and mirrors of the same episode still count once. Must keep the One Piece two-site case at ~1170.
5. `catalog_duration = 0` yields 0 minutes (`stats.rs:556`, `:1965`): treat `<= 0` as unknown.
6. `get_activity_years` not descending when a future year exists (`stats.rs:2580`): sort after inserting.
7. Popularity score divides by a magic 70,000 (`stats.rs:2554`): replace with the user's percentile against the local
   `anilist_catalog.popularity` distribution (exact, no constant). Changes the meter's meaning: label "más popular que el X % del catálogo".
8. `avg_episodes_per_series` divides by franchises, not series: rename the label to "por anime" or divide by series (pick label fix).

## B. Estimates shown as exact (PR 2, `feature/stats-estimate-marks`) — backend flag + front
1. Minutes watched: when any seen episode used a default duration (24/100/8/26), return `minutes_estimated_share`
   and show "≈" plus "N % estimado" instead of the current fixed help text (`es.ts:487`, which is wrong for OVA/short/linked rows).
2. Hero "Episodios vistos" (`StatsInsights.tsx:207`) adds real marks and "Ya lo vi" catalog totals: show real as the main
   number and "+N de «Ya lo vi»" as a secondary line (data already in `episodes_watched` / `episodes_watched_external`).
3. Next-episode dates rolled +7 d (`db/catalog.rs:1907`, `db/airing.rs:126`, `adapter/animeytx.rs:86`): return an
   `estimated: bool` with the date and mark it "≈ / estimado" in SeriesDetail, AiringGrid and the catalog. Not removed,
   because the Semana grid would lose the show for days.
4. Movie duration says "min por episodio" (`SeriesDetail.tsx:415`, `AnimeSheet.tsx:260`): "min" for MOVIE.
5. 3D graph invents 1 episode for "Ya lo vi" titles with no total (`StatsGraph.tsx:276`): use 0 / smallest node, not a fake count.

## C. Front number bugs (PR 3, `fix/front-number-bugs`), Vitest first
1. Heatmap month labels a week late when a month starts Tue-Sun (`stats/heatmapLayout.ts:126`, `break` after the first day).
2. Heatmap thresholds: few active days make the record day the faintest colour (`heatmapLayout.ts:30`, `count <= t1`).
3. `isWithinLastThreeMonths` uses `setMonth` and drifts on the 29th-31st (`AiringGrid.tsx:49`): fixed 91-day cutoff or clamp the day.
4. Library progress > 100 % and `===` misclassifies seen > total as watching (`Library.tsx:72`, `:157`): clamp, `>=`.
5. `formatMinutes` jumps (59.6 -> "60 min", 2879 -> "48 h", 2880 -> "2 d 0 h") (`StatsInsights.tsx:12`): compute from raw minutes.
6. `toFixed(1)` ignores es/ca decimal comma (`StatsInsights.tsx:431,437,458`): use the locale formatter.
7. Three countdown formatters disagree (AiringGrid "en 2 h", SeriesDetail "en 1 hora", catalog "1 h 45 min"): one shared helper.
8. SeriesDetail "4 / 4" vs Library "4 / 12" for an airing show: show "vistos / emitidos" with the planned total when known, in both.

## D. App data (PR 4, `fix/app-data-accuracy`)
1. `FuzzyDate::to_timestamp` drops year/month-only start dates (`anilist.rs:268`): also request AniList `season`/`seasonYear`
   and store them (exact) instead of inventing a day. Needs a column migration (supervisor writes it).
2. Linking can pick a movie/OVA of the same franchise (`matching.rs`): add `format` to the catalog index and prefer TV for scraped TV rows.
   Verify on real data first (read-only SELECT of linked series whose catalog format is MOVIE/OVA/SPECIAL) before coding.
3. Pending duplicates across sites when one row is linked and the other is not (`airing.rs:362`): verify on real data first
   (read-only), fix only if it happens.

## Rejected after checking
- "Discover 'Ya lo vi' inserts seen without `seen_at` is a critical bug" (`commands/discover.rs:161`): intentional — the
  watch date is unknown; stamping now would put fake activity on the heatmap. Same for the suggested
  `COALESCE(seen_at, added_at)` in charts: that would be a new estimate. Only an info line "N episodios vistos sin fecha" is worth adding.
- "`notifyNewEpisodes` is not wired": false, `App.tsx:282` calls it.
- "Pending hides episodes below the seen watermark": by design (watching is gap-free, CLAUDE.md).
- "Same-day completions should count as 1 day": deliberate (bulk marks), pinned by a test; no change.

## Order and delegation
A, C and D touch different files: three agy implement tracks in parallel (A on opus when quota is back, C and D flash);
B after A (shares stats.rs + StatsInsights). Supervisor: migrations, real-data read-only checks, builds, tests, review, PRs.

## Decisions for the user
1. Rolled next-air dates: mark as estimated (recommended) or hide?
2. Minutes watched with default durations: mark "≈ N % estimado" (recommended) or exclude unknown-duration episodes from the total?
3. Popularity meter as a catalog percentile (recommended) or keep the 0-10 scale?

## Merged (2026-10-31)
- A #128 stats backend (pending without «Ya lo vi», dusty by franchise with unseen episodes, cross-site progress =
  max(best site, distinct canonical episodes capped at the AniList total), duration 0, years order). Verified per
  franchise on a copy of the real DB.
- C #126 front number bugs (heatmap months/intensity, 3-month cutoff, progress clamp, time units, locale decimals).
- B #133 estimated-minutes share with "≈", «Ya lo vi» apart, popularity as catalog percentile, no fabricated graph
  episode. #136 next-air dates rolled +7 d marked "≈" (grid + detail), one shared countdown, "vistos / emitidos (plan)",
  movie duration.
- D #135 season markers anywhere in a title; startup repair takes season-N rows off season-1 entries (5 rows on the real
  DB: 2 relinked correctly, 2 unlinked, 1 newly linked).
- Also: #130 loading/progress motion + explicit `done` on refresh-progress.

## Left
- animeytx's own +1 week extrapolated dates are not marked (needs a DB column).
- AniList partial start dates (`FuzzyDate`) still dropped (needs a column + migration).
- Repair leaves some rows unlinked instead of relinking (resolve_catalog_link does not use the new season reading);
  "Re:Zero II: ..." multi-colon and "Part N" semantics noted in PR #135.
