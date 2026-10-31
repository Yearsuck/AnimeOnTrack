# Plan 2026-10-26: next-episode persistence, stats precision, airing trailer, site cleanup

Source: three read-only agy research reports (agy-airing-persist, agy-stats-trailer, agy-clean-audit).
Spot-checked by the supervisor: `get_next_air` reads ani.zip (`commands/catalog.rs:133-139`),
`list_airing` drops a catalog date `<= now` (`db/airing.rs:120`), stats "Ya lo vi" ignores rows
with `catalog_episodes = None` (`db/stats.rs:567-575`), status refresh cooldown is 6 h (`commands/catalog.rs:405`).

## A. Next-episode timing from AniList (PR 1, `feature/airing-anilist-timing`)
Findings (verified): three unrelated sources (ani.zip map in SeriesDetail, `anilist_catalog.next_airing_at`
in grid/catalog, scraped `series.next_episode_at` which only animeytx/gogoanime fill). Catalog value is
refreshed only at startup (6 h cooldown, max 500 rows); nothing refreshes while the app stays open, so after
an episode airs the show loses its countdown and drops out of the "Semana" grid. Long-runners are not in the
incremental sync partitions. Snapshot bootstrap imports past `next_airing_at`.
1. `get_next_air`: read `anilist_catalog(next_episode, next_airing_at)` first, ani.zip as fallback.
2. `stale_status_ids`: order followed/airing titles with `next_airing_at <= now` first (index-safe, test at real size).
3. In-session refresh: `refresh_catalog_status` on a 30-60 min timer / window focus; shorter cooldown when expired dates exist.
4. `SeriesDetail`: expired `next_airing_at` on a RELEASING title -> single-id refresh.
5. `list_airing`: a date passed < 7 days ago on a RELEASING title rolls +7 d until the refresh lands.
Open: flag the extrapolated date as approximate? Fuzzy-link followed series with `anilist_id` NULL (separate task).

## B. Stats precision (PR 2, `feature/stats-anilist-precision`)
Findings (verified): 24 min default, movie 100, short 8, OVA 26 only when `duration` is NULL. Airing shows
have `episodes = NULL`, so "Ya lo vi" credits 0 episodes; completion denominator = scraped rows only;
`next_episode` and `status` are stored but unused; backlog minutes use `max(duration)` of the franchise so
a movie prices every TV episode at 100 min; multi-season "Ya lo vi" takes `max(episodes)`, not the sum.
1. Add `catalog_next_episode`, `catalog_status` to `SeriesWatchRow`; airing + `episodes NULL` -> `next_episode - 1`.
2. Same lower bound in `canonical_franchise_episode_totals`.
3. Backlog per-episode duration from the TV member, not the franchise max.
4. DECISION NEEDED: multi-season "Ya lo vi": sum distinct AniList ids (fixes Mob Psycho S1-S3 = 36) vs keep `max()`
   (protects One Piece arc rows vs the 1140-episode entry). Default: keep `max()`.
Tests first for each (airing Ya-lo-vi, movie in franchise).

## C. Trailer in airing detail (PR 3, `feature/series-detail-trailer`)
Verified: backend already stores/returns the trailer; `SeriesDetail` never renders `extras.trailer`.
1. Extract `TrailerBox` + helpers from `AnimeSheet.tsx` to `src/components/TrailerBox.tsx`.
2. Move `.sheet-trailer*` rules (`styles/animesheet.css:161-231`) to a shared file imported by both views.
3. Render in `SeriesDetail` between `SeriesAbout` and `RelatedRow` (full width). CSP already allows YouTube/Dailymotion.

## D. Site cleanup (PR 4, `feature/clean-player-polish`)
Verified coverage: all 10 sites have selectors; animeland has no episode fixture (selectors are guesses);
aniwaves fixture is a 404 page; animoratv has no switcher.
1. `ad_block.rs`: add the hosts the agent found in fixtures (adsco.re, visariomedia.com, agl003.com, llvpn.com,
   numbestkarree.com, browsepantartful.com, cpmstar.com, yumenetworks.com, imasdk.googleapis.com, googletagmanager.com
   ...; full list in the clean-audit report). Re-check each in the fixture before adding; skip googletagmanager if it
   is required by a player.
2. `clean_player.rs`: relax the `z < 100` overlay gate (adangle), hit-test unblock at player centre,
   observer `misses >= 3` -> 10.
3. Selectors: gogoanime switcher `#w-servers .servers`; jkanime fallback `#video_box`; animeland container classes.
4. Capture real fixtures for animeland and aniwaves (needs the live site; separate step).

## Order and delegation
A and B touch different files from C and D: run 3 tracks (A, B+C sequential on stats vs front, D) as agy implement
agents, one worktree each, flash model (opus-4-6 quota resets ~5 h). Supervisor compiles, tests, reviews diff, commits, PRs.
