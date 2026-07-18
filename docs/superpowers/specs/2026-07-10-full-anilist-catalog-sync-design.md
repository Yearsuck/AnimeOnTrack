# Full AniList catalog sync (complete + fast + resumable)

Follow-up to the Catálogo tab work (commits `c18d506`, `456f421`, `40370d9`). The current `sync_anime_catalog` (`src-tauri/src/commands.rs`) paginates `Page(page, perPage: 50)` with `sort: POPULARITY_DESC` and a fixed 2.2s delay. Two problems:

1. **Incomplete**: AniList hard-caps offset pagination at `page * perPage <= 5000` per query. The sync stops at ~5000 titles; AniList hosts ~21,000 anime. `pageInfo.total` and `lastPage` are **fake** (always report 5000 / capped values — verified live 2026-07-10), so the current `estimated_total = last_page * PAGE_SIZE` progress math is also wrong.
2. **Slow for what it gets**: fixed 2.2s/request regardless of what `X-RateLimit-Remaining` says, no resume — an interrupted sync restarts from page 1.

## Verified API facts (curl against `https://graphql.anilist.co`, 2026-07-10)

- `X-Ratelimit-Limit: 30`, `X-Ratelimit-Remaining` headers present on every response (the API runs in a degraded 30 req/min state for **everyone** — authenticating does NOT raise it; do not add OAuth).
- There is **no `id_greater` cursor** on `Page.media` (returns `Unknown argument`). Offset pagination inside a filtered partition is the only option.
- `seasonYear` filter is **unreliable** (`seasonYear: 1917` returned 2017 movies — verified). Do not use it.
- `startDate_greater` / `startDate_lesser` (FuzzyDateInt, e.g. `19161231`) are **exact** — `startDate_greater: 19161231, startDate_lesser: 19180101` returns only year-1917 titles (verified).
- `status: NOT_YET_RELEASED` catches undated upcoming anime (verified — returns titles with `startDate.year: null`).
- `pageInfo.hasNextPage` **is** reliable within a partition; `total`/`lastPage` are not. Loop on `hasNextPage` only.
- `perPage` max is 50.

## Design

### Partitioned sync (`anilist.rs` + `commands.rs`)

Replace the single popularity-sorted crawl with a list of **partitions**, each guaranteed under the 5000-row cap, each paginated to exhaustion via `hasNextPage`:

1. One pre-modern range: `startDate_lesser: 19400101` (everything before 1940 — a few hundred titles total).
2. One partition per year from 1940 through current year + 2: `startDate_greater: {Y-1}1231, startDate_lesser: {Y+1}0101`. No single year approaches 5000 titles.
3. One `status: NOT_YET_RELEASED` partition (undated upcoming).
4. One final unfiltered `sort: ID` pass, first 100 pages (the 5000-cap worth) — catches undated non-upcoming leftovers among the oldest IDs. Residual gap (undated + not upcoming + id beyond first 5000) is accepted and documented.

All partitions use `sort: ID` (stable across pages — popularity ordering shifts live between requests and would skip/duplicate rows mid-partition). Duplicates across partitions are harmless: `upsert_catalog_anime` is keyed on AniList `id`.

Add `popularity` to the GraphQL field list and to `CatalogAnime`, because sync order no longer encodes popularity (see schema below).

### Adaptive rate pacing (`anilist.rs`)

`fetch_catalog_page` (or a new lower-level helper it and the partition crawl share) returns the parsed page **plus** the `X-RateLimit-Remaining` header value. Pacing rule in the sync loop:

- If `remaining >= 3`: no sleep (burst through the window).
- Otherwise sleep ~2100ms before the next request.
- On HTTP 429: honor `Retry-After` header (seconds), then retry the same page. Cap retries (e.g. 5) then fail the partition with a clear error.

Full catalog ≈ 21,000 / 50 ≈ 420 requests plus per-partition overhead ≈ ~470 requests → **~16 min floor** at 30 req/min. That floor is the API's, not ours; the win is completeness + resumability + not sleeping when the window has headroom.

### Resumable + incremental (`settings` table)

Persist sync state under settings key `catalog_sync_state` (JSON): list of completed partition labels + a `full_sync_completed_at` timestamp once all partitions finish.

- **Resume**: on `sync_anime_catalog`, skip partitions already marked complete. An interrupted sync continues where it left off.
- **Incremental re-sync**: if `full_sync_completed_at` is set, a new sync only runs: current year − 1 through current year + 2, plus `NOT_YET_RELEASED` (new/updated titles live there; back-catalog is static). A `force_full: bool` command arg clears the state and redoes everything.

### Schema (`db.rs`)

- `ensure_column(&self.conn, "anilist_catalog", "popularity", "INTEGER")` (existing migration helper — `CREATE TABLE IF NOT EXISTS` won't alter the existing table).
- `list_catalog` ordering changes from `ORDER BY sort_order` to `ORDER BY popularity DESC NULLS LAST, id` (`sort_order` no longer means popularity; keep the column, write the within-sync sequence into it, but stop ordering by it). Add an index: `CREATE INDEX IF NOT EXISTS idx_catalog_popularity ON anilist_catalog(popularity DESC)`.
- `upsert_catalog_anime` gains the `popularity` value.

### Progress reporting

Keep the `catalog-sync-progress` event but fix its payload semantics: `total` can't be known up front (API totals are fake), so emit `{ synced, total }` where `total` is a running best-estimate: `21000` hardcoded baseline for a full sync, or the real synced count once done; alternatively `{ synced, partition_label }`. Implementer picks whichever needs the least frontend change in `Catalog.tsx` / wherever the progress is consumed — but the UI must not show a bogus "5000 total".

## Acceptance criteria (verifiable)

1. `cargo test --manifest-path src-tauri/Cargo.toml` passes, including new unit tests: partition-list generation (pre-1940 range, per-year ranges, NYR, catch-all present; no year gap), sync-state JSON round-trip (resume skips completed partitions), `upsert_catalog_anime` with popularity.
2. After a full sync on the live API: `SELECT COUNT(*) FROM anilist_catalog` ≥ **20,000** (check via `sqlite3 %APPDATA%\com.ernes.aot-scaffold\animeontrack.sqlite`).
3. Full sync completes without a 429-triggered failure and in ≤ ~25 min.
4. Kill the app mid-sync, relaunch, re-trigger sync: it resumes (log/progress shows skipped partitions; total time noticeably shorter than starting over).
5. Second sync after completion takes ≤ ~2 min (incremental mode).
6. Catálogo tab still lists most-popular titles first (popularity ordering preserved via the new column, verified visually).
7. `npx tsc --noEmit` and `npm run build` pass.

## Live verification required

- Run the real full sync against AniList from the app (Catálogo tab button), screenshot the tab showing ≥20k synced and popular titles first.
- Verify the mid-sync kill/resume scenario for real, not just in unit tests.

## Explicitly out of scope

- OAuth/authenticated AniList access (verified: does not raise the rate limit).
- Syncing additional metadata fields beyond `popularity` (descriptions, tags, staff…).
- Any change to Descubrir/Catalog UI beyond progress-display correctness (tasks 2/3 cover those).
