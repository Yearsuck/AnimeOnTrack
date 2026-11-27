# Performance audit, 2026-11-27 (research only, nothing measured at runtime)

Scope: `src-tauri/src` (rusqlite behind one `Mutex<Db>`) and `src/` (React 19 + Vite).
Method: static reading only. No `node_modules` and no `target/` in the worktree, so `npm run build` and
`cargo test` were NOT run; every cost below is an estimate from reading code (UNCONFIRMED until measured).
Line numbers are for this worktree.

Already good (do not redo): WAL + `synchronous=NORMAL` + 32 MB cache (`db.rs:47-56`); heavy views are
`lazy()` (`App.tsx:50-75`); `three`/`react-force-graph-3d` have their own chunk (`vite.config.ts`);
`list_airing_season` releases the lock before normalising titles (`commands/scan.rs:299-318`);
scan listing is one transaction (`db/series.rs:136-149`); `hide_watched` was already fixed
(`db/catalog.rs:~912`); EXPLAIN-based guards exist (`db/airing/tests.rs:1066-1101`,
`db/catalog/tests.rs:~1913`).

## Ranking (impact / effort)

| # | Finding | Impact | Effort |
|---|---|---|---|
| 1 | Pendientes: ~5 SQL queries per followed series row, plus a full-table pass | High | Low-Med |
| 2 | `list_airing_season` re-reads and re-normalises the whole 22k-row catalog on every call | High | Low |
| 3 | Same `row_align` N+1 inside `sync_library_from_series` (runs at every startup and sync) | Med-High | Low (shares fix with 1) |
| 4 | Stats: `stats_genres(id)` called per series (N+1) while a batch version exists | Med | Low |
| 5 | Startup: `list_airing` called only as a "has source?" probe | Med | Very low |
| 6 | Catálogo genre filter uses NOCASE on a BINARY index, and `tags` has no index on `tag` | Med | Low |
| 7 | Catálogo page: 2 queries per row x 30 rows, 2 scans (list + count) per filter change | Low-Med | Low |
| 8 | Catálogo free-text search: 3 `LIKE '%x%'` columns + 3 subquery scans, run twice, lock held | Med (UNCONFIRMED) | Med |
| 9 | `list_pending` serialises the full `Series` once per episode | Low-Med | Med (API change) |
| 10 | `Pending` rebuilds list + refetches `getActiveSite` and `listPending` for every signal; `<img>` not lazy | Low-Med | Very low |
| 11 | `async` commands run blocking SQLite on tokio workers while holding the global mutex | Med (systemic) | Med |
| 12 | `@fontsource-variable/inter` default import bundles every subset; `library` table rewritten on each launch | Low | Very low |

### Recommended first PRs (small, independent): #1, #2, #4, #5, #6

---

## 1. Pendientes: per-member queries in `merged_pending_episodes_at`  [RECOMMENDED PR 1]

Evidence
- `db/airing.rs:594` calls `self.row_align(m.id, now)` for every candidate member. `row_align`
  (`db/episode_align.rs:81-124`) runs 5 statements each: `SELECT anilist_id FROM series WHERE id`
  (the caller already has `anilist_id` in `Row`), `episode_offset` (`db/catalog.rs:1898`),
  `COUNT(*) FROM anilist_episode_map ...` (`episode_align.rs:101`), and a full
  `SELECT number FROM episodes WHERE series_id` (`episode_align.rs:110`), although `eps_by_series`
  (`airing.rs:~520`) already holds those numbers in memory.
- `airing.rs:570` runs the same `COUNT(*) ... air_at <= now` again per group (duplicate of the one in `row_align`).
- Line 420-441: loads and `normalize_title`s ALL `series` rows (every site, followed or not) to build groups,
  although only groups containing a followed row matter.
- The UI calls `pending_count` (App badge, `App.tsx:212`) and `list_pending` (Pending view) on every
  `onPendingDataChanged`; both run the whole pipeline (`airing.rs:851-858`, `:859`). Each takes the mutex.

Cost: for F followed members with an AniList id: about 5F + G (groups) round trips, each a b-tree seek,
plus normalising S total series. With F=300 and S=6k that is roughly 1.5k statements and 6k normalisations per call,
done twice per change event. The existing test only guards "no table scan" (`airing/tests.rs:1066`),
not statement count.

Fix
1. Add `row_align_with(series_id, anilist_id, numbers: impl Iterator<Item=&str>, now, &cache)`; pass the
   `anilist_id` from `Row` and the already-loaded `eps_by_series` numbers (drops 2 of 5 queries).
2. Preload, once per call, `episode_offset` and `aired_count` for all relevant anilist ids with one
   `SELECT anilist_id, MIN(episode)... / SUM(air_at<=?) ... GROUP BY anilist_id` into a `HashMap`; reuse for `group_cap`
   (removes the rest, including the duplicate COUNT at line 570).
3. Restrict the initial `series` scan to groups with a followed row (two-pass: followed anilist ids and
   titles first), or keep the scan but skip `normalize_title` for series in no followed group.
4. Optionally make `pending_count` derive from a cached result of `list_pending` (see #9) rather than re-running.

Risk: behaviour must stay bit-identical (cumulative numbering heuristics, `RowAlign::decide`). Existing
tests (`airing/tests.rs`, `episode_align.rs` tests `db_row_align_with_tensei_fixture`) cover it; keep the old
`row_align` and add a differential test comparing old and new over the scaled fixture.

Measure: extend `pending_count_matches_list_pending_on_a_scaled_library` (`airing/tests.rs:1105`, 300 series
fixture) with a statement counter (`conn.profile` / `Connection::trace` callback counting) asserting O(1) statements
per call, plus an `Instant` print on a generated 6k-series / 120k-episode DB (run with `--nocapture`, do not assert on time).
Files: `src-tauri/src/db/airing.rs`, `src-tauri/src/db/episode_align.rs` (+ tests).

## 2. `list_airing_season` rebuilds the catalog title maps on each call  [RECOMMENDED PR 2]

Evidence: `commands/scan.rs:299-318`: each call does `catalog_airing_meta_rows()` (`db/catalog.rs:1159`:
`SELECT 7 cols FROM anilist_catalog`, about 22k rows including URL strings) and then
`build_catalog_airing_meta` (`catalog.rs:98`) which calls `normalize_title` up to 3x per row (about 66k calls) and
clones banner/cover strings into 4 maps. AiringGrid calls it on mount, on every `refreshSignal`
(`AiringGrid.tsx:240-243`) and throttled every 4 s during a catalog sync crawl (`AiringGrid.tsx:~245-290`).
The catalog only changes on sync/backfill.

Cost: tens of ms of lock-holding SELECT (22k rows) plus an estimated 20-80 ms CPU for normalisation, repeated on
every airing refresh (UNCONFIRMED, measure). During first-run crawl it repeats every 4 s.

Fix: cache `Arc<CatalogAiringMeta>` in `AppState` keyed by a cheap version (`SELECT COUNT(*), MAX(rowid)`, or a
`catalog_version` counter bumped by the sync/backfill writers in `db/catalog.rs`). Rebuild only on version change, in `spawn_blocking`.
Risk: stale cache if some catalog writer forgets to bump; mitigate by keying on `(COUNT(*), SUM(metadata_version), MAX(start_date))`
or the writers' single funnel. Measure: unit test timing `build_catalog_airing_meta` on 22k generated rows
(documents the saving), and a test asserting the second call does no `anilist_catalog` scan.
Files: `commands.rs` (AppState), `commands/scan.rs`, `db/catalog.rs`.

## 3. `sync_library_from_series` repeats the row_align N+1

Evidence: `db/library.rs:137` calls `self.row_align(row.id, now)` for every followed / watched / backlog row; plus
`canonical_group_keys()` over all series (`db/episodes.rs:399`, normalises every series and catalog title) and
`DELETE FROM library` + reinsert of every row (`library.rs:~176`). It runs at every `Db::open` (`db.rs:649`), in
`sync_state.rs:61` (each cloud sync), and in `follow.rs:493` and `:565` (library import). Opening is synchronous in
`setup` (`lib.rs:~150`), so it delays the first window.
Cost: about 4-5 queries x followed rows at each launch (UNCONFIRMED: with 300 followed, about 1.5k statements, likely 10-50 ms; grows with library).
Fix: reuse the batch `RowAlign` preload from #1 (a `row_aligns_for(&[(series_id, anilist_id)])` helper). Optionally skip the rewrite when the computed entries equal the stored table.
Risk: low, `library` tests cover grouping (`library.rs:838+`). Measure: same statement-counter approach; time `Db::open` on a generated DB.
Files: `db/library.rs`, `db/episode_align.rs` (shared with #1, so land after #1 or in the same PR).

## 4. Stats N+1: `stats_genres` per series  [RECOMMENDED PR 3]

Evidence: `db/stats.rs:711` (`get_genre_stats`), `:1184` (`get_stats_graph_data`), `:733`/`:768` (`taste_cards` via `labels_of`)
each call `stats_genres(id)` per member, which is `real_genres_batch(&[id])` (`stats.rs:792-794`) with a single id. The
batch API with a UNION and `prepare_cached` already exists (`stats.rs:800`, also used by Biblioteca).
Cost: 3 stats commands x (followed + watched series, for example 300-600) = 1-2k statements per Stats visit, each holding the
mutex on its own; the UNION with `NOT EXISTS` is heavier than a point lookup.
Fix: collect all member ids from `groups`, call `real_genres_batch(&all_ids)` once (chunk at 900 ids for the SQLite
variable limit, the function repeats the placeholder list so cap at about 450), then look up in the map.
Risk: low; output must stay identical (sorted, de-duplicated). Existing stats tests act as the oracle.
Measure: statement counter in a stats test over a 500-show fixture; EXPLAIN already index-based.
Files: `src-tauri/src/db/stats.rs` only.

## 5. Startup runs the whole airing pipeline as a probe  [RECOMMENDED PR 4]

Evidence: `App.tsx:239`: `await listAiring(); // throws if no source configured`. `list_airing` (`commands/scan.rs:402`) executes
`list_airing_with_rolled` (`db/airing.rs:70`): a query with two correlated subqueries per airing series
(`MAX(CAST(number AS REAL))`, `COUNT(*)` on episodes, `airing.rs:~89-90`), `season_consistent` per linked row,
`canonical_group_keys()` (all series), grouping and sorting; the result is discarded. Only afterwards does the view switch to
Pendientes, so this sits on the critical path before the first real screen. Pending then does its own `listPending`.
Cost: one extra full airing computation (estimate 20-100 ms with 1-2k airing rows, UNCONFIRMED) before first paint of the main view.
Fix: a tiny command `has_source()` (`state.source_id` is set, one mutex read, or `SELECT 1 FROM sources LIMIT 1`) used for the
decision; keep `listAiring` for AiringGrid. Also fire `refresh()`/Pending fetch without awaiting the probe.
Risk: very low, but check `get_source_id` error semantics (`commands.rs`) match the thrown condition.
Measure: time from window show to `loaded=true` in Pending (performance.mark in dev), or Rust-side timing in the command.
Files: `src-tauri/src/commands/scan.rs` (or `commands/library.rs`), `src-tauri/src/lib.rs` (register), `src/api.ts`, `src/App.tsx`.

## 6. Catálogo genre filter cannot use its index  [RECOMMENDED PR 5]

Evidence: `db/catalog.rs:~898-906`: `genre = ? COLLATE NOCASE` and `tag = ? COLLATE NOCASE`. The only genre index is
`idx_catalog_genre ON anilist_catalog_genres(genre)` with default BINARY collation (`db.rs:436`), so a NOCASE comparison
cannot use it (SQLite requires the index collation to match). `anilist_catalog_tags` has no index on `tag` at all
(PK is `(anilist_id, tag)`, `db.rs:342-346`). Each selected genre adds two uncorrelated subqueries that scan about
66k genre rows and about 170k tag rows (estimate: 22k titles x 3 genres and x 8 tags, UNCONFIRMED), done once for the page query
and again for `catalog_count_filtered` (`catalog.rs:~986`, called from `get_anime_catalog`, `commands/catalog.rs:184-185`).
Cost: a few tens of ms per filter change, per selected genre, with the mutex held.
Fix: `CREATE INDEX idx_catalog_genre_nocase ON anilist_catalog_genres(genre COLLATE NOCASE)` and
`idx_catalog_tags_tag_nocase ON anilist_catalog_tags(tag COLLATE NOCASE)` in the migration block at `db.rs:435`
(idempotent, `IF NOT EXISTS`); drop/replace the BINARY one only after checking nothing else relies on it.
Risk: very low (index only); small DB size growth (a few MB).
Measure: add an EXPLAIN QUERY PLAN test in `db/catalog/tests.rs` next to the existing one asserting `SEARCH ... USING (COVERING) INDEX`
for the genre and tag lookups; time `catalog_count_filtered` with `genres=["Action"]` on a generated 22k catalog.
Files: `src-tauri/src/db.rs`, `src-tauri/src/db/catalog/tests.rs`.

## 7. Catálogo page: N+1 for genres/tags and double scan

Evidence: `db/catalog.rs:~970-974`: `for anime in &mut rows { list_catalog_genres(id); list_catalog_tags(id) }`, and both helpers use
non-cached `prepare` (`catalog.rs:1369-1385`). 30 rows per page (`commands/catalog.rs:180`) = 60 statements + 60 compiles. The page
query and `COUNT(*)` share one WHERE, so any filter change scans twice. Sorting by `average_score`/`start_date`/`title`
has no index (only `popularity DESC`, `db.rs:435`), so those sorts filesort all matching rows (22k at most).
Cost: small per call (about 1-3 ms for 60 prepared statements, a filesort of 22k rows is about 10-30 ms, UNCONFIRMED), but it stacks with #6/#8 under the lock.
Fix: use `prepare_cached`, or one `WHERE anilist_id IN (<30 ids>)` query per table grouped in Rust; optionally add `idx_catalog_score(average_score DESC)`
and `idx_catalog_start_date(start_date DESC)` if measurements justify. Risk: low. Measure: timing test for `list_catalog_filtered` page 1 per sort on 22k generated rows; EXPLAIN for ORDER BY (`USE TEMP B-TREE FOR ORDER BY` disappears).
Files: `db/catalog.rs`, optionally `db.rs`.

## 8. Catálogo free-text search

Evidence: `db/catalog.rs:838-843`: `title`, `title_romaji`, `title_english` `LIKE '%q%' COLLATE NOCASE` (full scan, no index
possible) OR-ed with three `id IN (SELECT ... LIKE '%q%')` subqueries over tags (about 170k rows), genres and synonyms. Debounce is 300 ms
(`Catalog.tsx:319`), but every settled query runs list + count (2 scans) under the single mutex, blocking every other command
(badge refresh, Pendientes, scan) while it runs.
Cost: UNCONFIRMED; plausibly 50-150 ms per search on a full catalog. Not a freeze, but a pause for concurrent commands.
Fix options (decide after measuring): (a) drop the tags/synonyms subqueries for short queries; (b) FTS5 virtual table over titles/synonyms
(bigger change, needs rusqlite `bundled` FTS5, which is on by default in the bundled build, verify); (c) cache the count per filter signature.
Risk: Med for (b), behaviour change in matching. Measure: timing test over 22k generated catalog with `search="nar"`.
Files: `db/catalog.rs`, `db.rs`.

## 9. `list_pending` payload repeats the series per episode

Evidence: `db/airing.rs:983-986` pushes `(group.series.clone(), ep)` for each episode, and `commands/library.rs:~27` wraps each as `PendingItem { series, episode }`,
so each episode serialises its show's `title`, `url`, `slug`, `cover_url` (which can be a long `data:` URL; the SQL explicitly allows
`data:%`, `airing.rs:~876`). A show with 40 pending episodes ships 40 copies. The frontend re-groups by `series.id` (`Pending.tsx:264`).
Cost: JSON size scales with episodes x cover size; 500 pending episodes with a 100 KB data URL cover each would be tens of MB across IPC (UNCONFIRMED
whether data: covers occur in practice, check `cover_cache.rs`; `file:` URLs are small). Serialisation also happens on the async worker.
Fix: return `Vec<{series, episodes[]}>` (grouped) and adapt `Pending.tsx`; or skip serialising the cover when identical. Risk: Med, a command-contract change touching `types.ts`, `api.ts`, `Pending.tsx`, and any test using `PendingItem`.
Measure: `serde_json::to_vec(&list_pending(...)).len()` in a test on the scaled fixture before and after.
Files: `commands/library.rs`, `db/airing.rs`, `src/types.ts`, `src/api.ts`, `src/views/Pending.tsx`.

## 10. Pending view refetching and image loading

Evidence: `Pending.tsx:62-67` calls `getActiveSite()` on every `refreshSignal`, `:118-123` calls `listPending(sort)` on every signal
and also resets `sourcesCache` (`:111-112`). `onPendingDataChanged` (`App.tsx:~230`) always does pendingCount + signal, so every seen toggle
costs two full pipelines (see #1) and a full list re-render. `<img>` at `Pending.tsx:25` has no `loading="lazy"` / `decoding="async"` (AiringGrid and PosterCard have lazy);
other eager images: `StatsGraph.tsx:1085`, `ListasDenseRow.tsx:120`, `HistoryRow.tsx:28`. There is no list virtualisation anywhere; Pending collapses to 8 eps per show
(`COLLAPSED_EPS`) so DOM size is bounded by shows x 8, acceptable. The `.map` rows are not memoised (`Pending.tsx:264-293`).
Cost: low-med; with 100 followed shows about 100 posters decoded eagerly. UNCONFIRMED visible jank.
Fix: add `loading="lazy" decoding="async"` and width/height to the poster; fetch `activeSiteId` once (it only changes on site switch); extract the show row to a `React.memo` component; keep optimistic local updates instead of refetching for a single seen toggle where safe.
Risk: very low for img attrs; Low-Med for refetch changes (the signal exists to fix real drift, see the comment at `Pending.tsx:33-40`). Measure: React Profiler commit duration with 100 shows; count of `listPending` invocations per user action (log in `api.ts`).
Files: `src/views/Pending.tsx` (+ trivial `StatsGraph.tsx`, `listas/ListasDenseRow.tsx`).

## 11. Blocking SQLite inside `async` commands, single global lock

Evidence: nearly all commands are `pub async fn` that call `state.with_db(...)` or `state.db.lock().unwrap()` synchronously
(`commands/library.rs:5-47`, `commands/stats.rs:15-104`, `commands/scan.rs:402-405`). That blocks a tokio worker for the whole query and queues
every other DB command behind one `Mutex<Db>`. Stats triggers about 12 such commands at once, each sequential on the mutex. Heavy paths (#1, #8, `list_airing`) therefore delay trivial ones
such as `pending_count` and settings reads. Long scan phases are well scoped (lock released around network awaits), so this is mainly about read latency, not freezes.
Cost: systemic head-of-line blocking; magnitude depends on #1/#6/#8. UNCONFIRMED in practice.
Fix (incremental): first fix the heavy queries; then, if still needed, a second read-only connection (WAL allows concurrent readers) in `AppState` used by pure read commands (`list_pending`, `list_library`, catalog list, stats), wrapped in `spawn_blocking`. Risk: Med (two connections, `:memory:` tests need a shared-cache URI, cache_size doubling). Do not start here.
Measure: log lock wait time in `with_lock` (`commands.rs:~165`) behind a debug flag and look at p95 under a refresh.
Files: `commands.rs` (+ later per-command files).

## 12. Minor

- `main.tsx:13` `import "@fontsource-variable/inter"` pulls the CSS for all subsets (latin, latin-ext, cyrillic, greek, vietnamese). Files load lazily by `unicode-range`, so impact is bundle/CSS size, not runtime; import `@fontsource-variable/inter/wght.css` is the same, so use `.../latin.css` (and `latin-ext.css`) for a smaller dist. Measure with `npm run build` (dist size). File: `src/main.tsx`.
- `db.rs:~416` the `UPDATE anilist_catalog SET metadata_version = 1 WHERE metadata_version = 0 AND ...` and the duplicate-episode probe `GROUP BY series_id, number` (`db.rs:~592`; the UNIQUE index is `(series_id,url)`, so it sorts through a temp b-tree) run on every launch. Both are single scans of 22k / ~100k rows, likely under 50 ms; only worth touching if startup profiling shows them. UNCONFIRMED.
- `first_episode_dates` (`db/episodes.rs:189-209`) uses `ORDER BY CAST(e2.number AS INTEGER)` per airing series, which sorts each series' episodes (cheap per series, N series). Combined into #1/#2 follow-up if airing refresh is still slow.
- `list_airing_with_rolled` correlated subqueries (`airing.rs:~89-90`) could be one `LEFT JOIN (SELECT series_id, MAX(...), COUNT(*) ... GROUP BY series_id)`. Same table, two scans of each series' episodes. Minor win.

## Startup path (item 2 of the brief)

Before the window is usable (`lib.rs:130-258`): `migrate_legacy_app_data`, `apply_pending_restore`, `Db::open` (WAL pragma, `init_schema`
with about 60 `ensure_column` PRAGMA table_info calls, two domain migrations, `repair_scraped_synthetic_rows`, duplicate probe,
`sync_library_from_series` (#3), `reconcile_completion_across_sites`), `clear_interrupted_downloads`, `sweep_partial_downloads`, window fit. Everything runs
synchronously on the setup thread. Deferred correctly: auto backup (spawn), auto sync (15 s delay), anizip sync, ad-block engine (background).
Frontend then: probe `listAiring` (#5), `Pending` fetch, `refresh()` which rescans the site and then reloads Pending a second time (`App.tsx:244-247`).
Best wins: #3 and #5. Moving `sync_library_from_series` out of `Db::open` into a deferred task is possible but changes the data-consistency guarantee at first paint, so do not do it without a decision.

## Existing tooling and how to extend it

- EXPLAIN tests: `db/airing/tests.rs:1066-1101` (pending lookups), `db/catalog/tests.rs:~1913` (`STALE_STATUS_IDS_SQL`). Pattern: run `EXPLAIN QUERY PLAN`, assert all rows start with `SEARCH`/contain the index name. Add the same for the genre/tag filter (#6) and the catalog sort.
- Scaled correctness fixture: `db/airing/tests.rs:1105` (`pending_count_matches_list_pending_on_a_scaled_library`, 300 series). No wall-clock assertions (deliberately removed per the comment at `:1058`). Extend with a statement counter (rusqlite `Connection::profile`) to guard N+1 regressions without timing flakiness.
- No benchmark crate (`criterion`) and no frontend perf test; `vitest` only. Bundle: `vite.config.ts` manualChunks (graph3d, vendor). Run `npm ci && npm run build` and record chunk sizes before any frontend PR (not done here, no node_modules).
