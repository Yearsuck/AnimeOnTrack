use super::*;

/// Full genre list for one series, for the Listas view's "Quiero ver" rows.
#[tauri::command]
pub async fn get_series_genres(state: State<'_, AppState>, series_id: i64) -> Result<Vec<String>, String> {
    let db = state.db.lock().unwrap();
    db.list_series_genres(series_id).map_err(|e| e.to_string())
}

/// Bulk-fetch genre lists for multiple series ids in one call.
#[tauri::command]
pub async fn get_series_genres_batch(
    state: State<'_, AppState>,
    series_ids: Vec<i64>,
) -> Result<std::collections::HashMap<i64, Vec<String>>, String> {
    let db = state.db.lock().unwrap();
    db.list_series_genres_batch(&series_ids).map_err(|e| e.to_string())
}


/// This user's top `limit` genres by affinity score (positive only), for
/// the "Tus géneros favoritos" chip row in the swipe UI — makes the
/// otherwise-invisible taste-weighting in `discover_swipe_card` visible.
///
/// Deliberately **not** scoped to the active site: this is a summary of the
/// user's whole library, like every other stats-shaped figure in the app. It
/// used to reuse the deck's own `get_genre_affinity(source_id)` — a query
/// correctly scoped to one site for picking which of *that* site's genre pages
/// to browse — which silently dropped the taste evidence sitting on the other
/// sites. See `Db::get_genre_affinity_across_sites`.
#[tauri::command]
pub async fn get_top_genres(state: State<'_, AppState>, limit: usize) -> Result<Vec<GenreAffinity>, String> {
    let db = state.db.lock().unwrap();
    db.get_favorite_genres(limit).map_err(|e| e.to_string())
}

/// Full matched AniList catalog entry for a series (cover/genres/format/
/// episodes/score/studio/url) — SeriesDetail's info panel source now that
/// anime metadata comes from AniList rather than the scraped site. `None`
/// when nothing in the synced catalog matches.
#[tauri::command]
pub async fn get_catalog_info_for_series(
    state: State<'_, AppState>,
    series_id: i64,
) -> Result<Option<crate::anilist::CatalogAnime>, String> {
    let db = state.db.lock().unwrap();
    db.catalog_info_for_series(series_id).map_err(|e| e.to_string())
}

#[derive(Serialize)]
pub struct SeriesSiteInfo {
    /// Display name of the site this row was scraped from.
    pub site_name: Option<String>,
    /// The site's own synopsis (usually Spanish), when a scrape captured it.
    pub synopsis: Option<String>,
}

/// Site-side facts for the detail view: which site the row belongs to (for
/// an honestly labelled "open on <site>" link) and its synopsis. Local only.
#[tauri::command]
pub async fn get_series_site_info(state: State<'_, AppState>, series_id: i64) -> Result<SeriesSiteInfo, String> {
    let db = state.db.lock().unwrap();
    Ok(SeriesSiteInfo {
        site_name: db.series_site_name(series_id).map_err(|e| e.to_string())?,
        synopsis: db.series_synopsis(series_id).map_err(|e| e.to_string())?,
    })
}

#[derive(Serialize)]
pub struct AnimeExtras {
    /// Raw AniList description markup; the frontend renders it as plain text.
    pub description: Option<String>,
    pub relations: Vec<crate::db::extras::RelatedAnime>,
    pub trailer: Option<crate::anilist::Trailer>,
}

/// How long a fetched description/relation list is trusted. Both change
/// rarely (a new sequel announcement is the usual reason).
const MEDIA_EXTRAS_MAX_AGE_SECS: i64 = 30 * 24 * 3600;

/// AniList description + related titles for the detail view. Served from the
/// `anilist_media_extras` cache; one on-demand AniList request when the entry
/// is missing or older than `MEDIA_EXTRAS_MAX_AGE_SECS`. The db lock is never
/// held across the request; a failed request falls back to the stale cache
/// and to the PREQUEL/SEQUEL edges the catalog sync already stored.
#[tauri::command]
pub async fn get_anime_extras(state: State<'_, AppState>, anilist_id: i64) -> Result<AnimeExtras, String> {
    if anilist_id <= 0 {
        return Ok(AnimeExtras { description: None, relations: Vec::new(), trailer: None });
    }
    let now = chrono::Utc::now().timestamp();
    let cached = state.db.lock().unwrap().cached_media_extras(anilist_id).map_err(|e| e.to_string())?;
    let extras = match cached {
        Some((extras, fetched_at)) if now - fetched_at < MEDIA_EXTRAS_MAX_AGE_SECS => Some(extras),
        // A recent failure or a request already in flight for this id: serve
        // what is cached instead of starting another request chain.
        stale if !state.extras_guard.try_begin(anilist_id, now) => stale.map(|(extras, _)| extras),
        stale => {
            let result = crate::anilist::fetch_media_extras(anilist_id).await;
            state.extras_guard.finish(anilist_id, chrono::Utc::now().timestamp(), result.is_err());
            match result {
                Ok(fetched) => {
                    let db = state.db.lock().unwrap();
                    if let Err(e) = db.store_media_extras(anilist_id, fetched.as_ref(), now) {
                        eprintln!("[anilist] caching extras for {anilist_id} failed: {e}");
                    }
                    fetched
                }
                Err(e) => {
                    eprintln!("[anilist] extras for {anilist_id} unavailable: {e}");
                    stale.map(|(extras, _)| extras)
                }
            }
        }
    };
    let (description, fetched_relations, trailer) = match extras {
        Some(e) => (e.description, e.relations, e.trailer),
        None => (None, Vec::new(), None),
    };
    let db = state.db.lock().unwrap();
    let relations = db.related_anime(anilist_id, &fetched_relations).map_err(|e| e.to_string())?;
    Ok(AnimeExtras { description, relations, trailer })
}

#[derive(serde::Serialize)]
pub struct NextAir {
    pub episode: i64,
    pub air_at: i64,
}

/// Next upcoming episode and its air timestamp (in seconds) for `anilist_id`
/// from the stored ani.zip episode map. Returns None when no upcoming episodes exist.
#[tauri::command]
pub async fn get_next_air(state: State<'_, AppState>, anilist_id: i64) -> Result<Option<NextAir>, String> {
    let now = chrono::Utc::now().timestamp();
    let db = state.db.lock().unwrap();
    let next = db.next_airing_episode(anilist_id, now).map_err(|e| e.to_string())?;
    Ok(next.map(|(episode, air_at)| NextAir { episode, air_at }))
}

#[derive(Serialize)]
pub struct CatalogPage {
    pub items: Vec<crate::anilist::CatalogAnime>,
    pub has_next_page: bool,
    pub total_synced: i64,
    pub total_matching: i64,
}

/// One page of the locally-synced AniList catalog — the "Catálogo" tab's
/// data source, independent of anything scraped off the tracked site. Reads
/// the local `anilist_catalog` table (see `sync_anime_catalog`), not
/// AniList live — instant, no rate-limit exposure, works with whatever's
/// been synced so far even mid-sync. `filter` narrows the result (search,
/// genres, format, score, episode bucket — see `crate::db::CatalogFilter`);
/// `total_synced` stays the unfiltered grand total (header text) while
/// `total_matching` reflects `filter` and drives `has_next_page` / the
/// "N resultados" label.
#[tauri::command]
pub async fn get_anime_catalog(
    state: State<'_, AppState>,
    page: i64,
    filter: Option<crate::db::CatalogFilter>,
) -> Result<CatalogPage, String> {
    let db = state.db.lock().unwrap();
    let per_page = 30;
    let filter = filter.unwrap_or_default();
    let items = db.list_catalog_filtered(page, per_page, &filter).map_err(|e| e.to_string())?;
    let total_synced = db.catalog_count().map_err(|e| e.to_string())?;
    let total_matching = db.catalog_count_filtered(&filter).map_err(|e| e.to_string())?;
    let has_next_page = page * per_page < total_matching;
    Ok(CatalogPage { items, has_next_page, total_synced, total_matching })
}

#[derive(Serialize)]
pub struct CatalogFacets {
    pub genres: Vec<String>,
    pub formats: Vec<String>,
    pub studios: Vec<String>,
}

/// Distinct genre/format/studio vocabularies from the synced catalog — drives
/// the Catálogo filter bar's chips/selects without hardcoding AniList's own
/// vocabulary (which can grow over time). Cheap `SELECT DISTINCT`s; the
/// frontend loads this once per mount, tolerating staleness until the next
/// sync (facets only change after a re-sync).
#[tauri::command]
pub async fn get_catalog_facets(state: State<'_, AppState>) -> Result<CatalogFacets, String> {
    let db = state.db.lock().unwrap();
    let genres = db.distinct_catalog_genres().map_err(|e| e.to_string())?;
    let formats = db.distinct_catalog_formats().map_err(|e| e.to_string())?;
    let studios = db.distinct_catalog_studios().map_err(|e| e.to_string())?;
    Ok(CatalogFacets { genres, formats, studios })
}

#[derive(Serialize, Clone)]
struct CatalogSyncProgress {
    synced: i64,
    total: i64,
}

/// Fetch AniList's full anime catalog and store it locally. Replaces a
/// single popularity-sorted crawl (which silently capped out around 5000
/// titles — AniList hard-limits `page * perPage <= 5000`, and
/// `pageInfo.total`/`lastPage` always report a fake fixed value, verified
/// live) with a list of date/status **partitions** each guaranteed to stay
/// under that cap, crawled to exhaustion via the one reliable signal,
/// `pageInfo.hasNextPage` — see `anilist::build_partitions` for the
/// partition scheme and why (year buckets, not `seasonYear` or a cursor
/// that doesn't exist on this API).
///
/// State (`settings` key `catalog_sync_state`) persists which partitions
/// have finished so an interrupted sync resumes instead of restarting, and
/// once a full sync has completed once, subsequent syncs only re-run the
/// window where titles actually change (current year ± a couple, plus
/// upcoming) — see `anilist::{build_partitions, incremental_partitions}`.
/// `force_full=true` clears that state and redoes everything from scratch.
///
/// Pacing: paces almost every request (~2.1s apart, ~28.6 req/min) and only
/// skips the sleep for the handful of requests right after AniList's
/// rate-limit window (`X-Ratelimit-Remaining`, 30/min unauthenticated) has
/// just reset. An earlier version skipped sleeping whenever `remaining >= 3`
/// to "burst through headroom" — live testing (2026-07-10) showed this
/// blows through most of a window in a few seconds of request/response
/// round-trips, then stays rate-limited for the rest of that ~60s window
/// regardless of per-request sleep length (the window doesn't refill
/// per-request like a token bucket), which surfaced as a partition failing
/// after exhausting its 429 retries. A 429 is still honored via
/// `Retry-After` inside `fetch_partition_page` as a safety net.
///
/// This is a real multi-minute job for the ~21,000-title full catalog, so
/// it's user-triggered from the Catálogo tab, not run automatically on
/// every app launch.
#[tauri::command]
pub async fn sync_anime_catalog(
    app: AppHandle,
    state: State<'_, AppState>,
    force_full: bool,
) -> Result<i64, String> {
    run_catalog_sync(app, state, force_full).await
}

/// Auto-triggered counterpart to the manual "Sync" button in the Catálogo
/// tab. `list_airing_season` (see `db::catalog::catalog_start_dates_by_normalized_title`)
/// depends on `anilist_catalog.start_date` to resolve "Esta temporada" for
/// series the user doesn't follow (no scraped episode dates for those) — but
/// a full sync only happens once, on first launch, and incremental syncs are
/// otherwise only reachable by the user manually opening Catálogo and
/// clicking Sync. Without this, `start_date` silently never backfills for
/// anyone who ran their first full sync before the column existed, and
/// "Esta temporada" quietly drops every unfollowed airing show. Called
/// fire-and-forget from the frontend on startup; throttled via
/// `catalog_auto_sync_last_at` so it doesn't re-run (and re-hit AniList's
/// rate limit) on every launch — see `should_auto_sync_catalog`.
///
/// The cooldown is stamped **after** the sync, and only when it succeeded (see
/// `should_stamp_auto_sync`). Stamping up front meant a transient failure —
/// AniList down, rate-limited, or simply no network at launch — burned the
/// whole ~12h cooldown even though nothing had synced, so the retry the user
/// would naturally trigger by relaunching the app was skipped for the rest of
/// the day.
#[tauri::command]
pub async fn maybe_sync_catalog_incremental(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Option<i64>, String> {
    let last_at = {
        let db = state.db.lock().unwrap();
        db.get_setting("catalog_auto_sync_last_at").map_err(|e| e.to_string())?
    };
    if !should_auto_sync_catalog(last_at.as_deref(), chrono::Utc::now()) {
        return Ok(None);
    }
    // `state` is consumed by the sync; keep a handle to reach `AppState` again
    // afterwards to record the outcome.
    let app_after = app.clone();
    let result = run_catalog_sync(app, state, false).await;
    if should_stamp_auto_sync(&result) {
        // Stamped at completion, not at start: the cooldown is "how long since
        // the catalog was last actually brought up to date", and a full
        // incremental pass is a multi-minute job.
        let state = app_after.state::<AppState>();
        let db = state.db.lock().unwrap();
        db.set_setting("catalog_auto_sync_last_at", &chrono::Utc::now().to_rfc3339())
            .map_err(|e| e.to_string())?;
    }
    result.map(Some)
}

/// Pure throttle check for `maybe_sync_catalog_incremental`: run once per
/// `AUTO_SYNC_COOLDOWN_SECS`, or immediately if never run / the persisted
/// timestamp is unparseable (treat corrupt state as "never run" rather than
/// getting stuck skipping forever).
pub fn should_auto_sync_catalog(last_at: Option<&str>, now: chrono::DateTime<chrono::Utc>) -> bool {
    const AUTO_SYNC_COOLDOWN_SECS: i64 = 12 * 60 * 60;
    match last_at.and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok()) {
        Some(last) => (now - last.with_timezone(&chrono::Utc)).num_seconds() >= AUTO_SYNC_COOLDOWN_SECS,
        None => true,
    }
}

/// Whether an auto-sync attempt's outcome should write
/// `catalog_auto_sync_last_at` — i.e. start the cooldown. Only a success does:
/// the cooldown exists to stop us hammering AniList after a *real* sync, and a
/// failed attempt synced nothing, so it has nothing to cool down from.
pub fn should_stamp_auto_sync<T, E>(outcome: &Result<T, E>) -> bool {
    outcome.is_ok()
}

/// Resolve engaged-but-unlinked series to their AniList catalog row using only
/// the local catalog — no network, no scraping. See `Db::link_series_to_catalog`
/// for the actual matching logic (factored out so `scan_airing_via_mirrors`
/// can also call it, on every site switch, not just at app startup).
#[tauri::command]
pub async fn link_series_to_catalog(state: State<'_, AppState>) -> Result<i64, String> {
    // Read the titles under the lock, build the (CPU-heavy) index without it,
    // then take the lock again only to link — see `link_series_to_catalog_with`.
    let rows = {
        let db = state.db.lock().unwrap();
        db.catalog_titles_for_index().map_err(|e| e.to_string())?
    };
    let (index, rows) = tauri::async_runtime::spawn_blocking(move || {
        let index = crate::matching::CatalogIndex::build(&rows);
        (index, rows)
    })
    .await
    .map_err(|e| e.to_string())?;
    let db = state.db.lock().unwrap();
    db.link_series_to_catalog_with(&index, rows).map_err(|e| e.to_string())
}

#[derive(Serialize, Clone)]
struct CatalogBackfillProgress {
    done: i64,
    total: i64,
}

/// Re-fetch catalog rows stored before `title_romaji`/`duration`/`status`/
/// `studio`/`start_date` were part of the sync query, filling those columns in
/// place. Which rows those are comes from `db::stale_catalog_ids`, i.e. from
/// each row's `metadata_version` stamp rather than from guessing at which
/// columns happen to be NULL — see `db::catalog::CATALOG_METADATA_VERSION`.
/// Writing a row back through `upsert_catalog_anime_batch` re-stamps it, so a
/// backfilled row drops out of the stale set on the next run.
///
/// Needed because none of the existing sync paths ever revisit them: a full
/// sync runs once and then records itself complete, and the incremental sync
/// only re-crawls the current year ± a couple. On a database whose first full
/// crawl predates those fields that leaves tens of thousands of rows with
/// NULLs forever — which the stats screen feels directly, since a "Ya lo vi"
/// title with no `episodes`/`duration` contributes nothing at all to watch
/// time or episode totals.
///
/// Paced and resumable exactly like `run_catalog_sync`: rows are refetched
/// worst-first (linked to a series, then most popular), and each batch is
/// committed before the next request, so an interrupted run keeps everything
/// it already fixed and the next run simply sees fewer stale ids.
#[tauri::command]
pub async fn backfill_catalog_metadata(app: AppHandle, state: State<'_, AppState>) -> Result<i64, String> {
    static RUNNING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if RUNNING.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return Ok(0);
    }
    let _guard = crate::commands::follow::RunningGuard(&RUNNING);

    // Same ~28.6 req/min pacing as `run_catalog_sync`, and unconditional here:
    // every request in this loop is one full 50-row batch, so there are never
    // cheap pages worth spending fresh-window headroom on.
    const PACED_SLEEP: std::time::Duration = std::time::Duration::from_millis(2100);

    let stale_ids = {
        let db = state.db.lock().unwrap();
        db.stale_catalog_ids().map_err(|e| e.to_string())?
    };
    let total = stale_ids.len() as i64;
    let mut done = 0i64;
    let _ = app.emit("catalog-backfill-progress", CatalogBackfillProgress { done, total });

    for batch in stale_ids.chunks(crate::anilist::MAX_IDS_PER_REQUEST * crate::anilist::PAGES_PER_REQUEST as usize) {
        let started = std::time::Instant::now();
        let fetched = crate::anilist::fetch_by_ids_multi(batch).await.map_err(|e| e.to_string())?;
        {
            let db = state.db.lock().unwrap();
            let mut items = Vec::with_capacity(fetched.len());
            for anime in &fetched {
                let sort_order = db.catalog_sort_order(anime.id).map_err(|e| e.to_string())?;
                items.push((anime, sort_order));
            }
            db.upsert_catalog_anime_batch(&items).map_err(|e| e.to_string())?;
        }
        // Counted over the batch, not the response: ids AniList no longer
        // serves (deleted/merged entries) never come back, and counting only
        // what returned would leave the progress bar permanently short of its
        // total and re-request those same dead ids on every future run.
        done += batch.len() as i64;
        let _ = app.emit("catalog-backfill-progress", CatalogBackfillProgress { done, total });
        // The request itself already used part of the 2.1 s slot.
        tokio::time::sleep(PACED_SLEEP.saturating_sub(started.elapsed())).await;
    }
    Ok(done)
}

/// Pure throttle check for `refresh_catalog_status`: run once per 6 hours,
/// or immediately if never run / persisted timestamp is unparseable.
pub fn should_refresh_catalog_status(last_at: Option<&str>, now: chrono::DateTime<chrono::Utc>) -> bool {
    const STATUS_REFRESH_COOLDOWN_SECS: i64 = 6 * 60 * 60;
    match last_at.and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok()) {
        Some(last) => (now - last.with_timezone(&chrono::Utc)).num_seconds() >= STATUS_REFRESH_COOLDOWN_SECS,
        None => true,
    }
}

/// Keep AniList catalog `status` fresh for shows the user cares about (airing,
/// releasing, or recently finished). Batches candidate ids (from `db.stale_status_ids()`)
/// through AniList's GraphQL API, preserving existing `sort_order`, with ~2100ms
/// pacing between batches. Throttled via settings key `catalog_status_refreshed_at`
/// to run at most once every 6 hours. After updating catalog rows, runs
/// `sync_status_from_catalog()` so linked series have their `is_airing`
/// flag corrected when AniList reports them as finished.
#[tauri::command]
pub async fn refresh_catalog_status(state: State<'_, AppState>) -> Result<Option<i64>, String> {
    static RUNNING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if RUNNING.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return Ok(None);
    }
    let _guard = crate::commands::follow::RunningGuard(&RUNNING);

    const PACED_SLEEP: std::time::Duration = std::time::Duration::from_millis(2100);

    let last_at = {
        let db = state.db.lock().unwrap();
        db.get_setting("catalog_status_refreshed_at").map_err(|e| e.to_string())?
    };
    if !should_refresh_catalog_status(last_at.as_deref(), chrono::Utc::now()) {
        return Ok(None);
    }

    let stale_ids = {
        let db = state.db.lock().unwrap();
        db.stale_status_ids().map_err(|e| e.to_string())?
    };

    let mut done = 0i64;
    for batch in stale_ids.chunks(crate::anilist::MAX_IDS_PER_REQUEST * crate::anilist::PAGES_PER_REQUEST as usize) {
        let started = std::time::Instant::now();
        let fetched = crate::anilist::fetch_by_ids_multi(batch).await.map_err(|e| e.to_string())?;
        {
            let db = state.db.lock().unwrap();
            let mut items = Vec::with_capacity(fetched.len());
            for anime in &fetched {
                let sort_order = db.catalog_sort_order(anime.id).map_err(|e| e.to_string())?;
                items.push((anime, sort_order));
            }
            db.upsert_catalog_anime_batch(&items).map_err(|e| e.to_string())?;
        }
        done += batch.len() as i64;
        tokio::time::sleep(PACED_SLEEP.saturating_sub(started.elapsed())).await;
    }

    {
        let db = state.db.lock().unwrap();
        db.set_setting("catalog_status_refreshed_at", &chrono::Utc::now().to_rfc3339())
            .map_err(|e| e.to_string())?;
        db.sync_status_from_catalog().map_err(|e| e.to_string())?;
    }

    Ok(Some(done))
}

/// Where a new install fetches the pre-built catalog snapshot from (a GitHub
/// Release asset kept up to date by the owner). `AOT_CATALOG_SNAPSHOT_URL`
/// overrides it for development; an override skips the "public host" check so
/// a local file server can be used.
const SNAPSHOT_URL: &str =
    "https://github.com/Yearsuck/AnimeOnTrack/releases/download/catalog-snapshot/catalog-snapshot.json.gz";
/// A catalog this small is treated as "new install"; anything bigger already
/// has data the snapshot must not second-guess.
const BOOTSTRAP_MAX_EXISTING: i64 = 1_000;
const MAX_SNAPSHOT_DOWNLOAD_BYTES: usize = 64 * 1024 * 1024;

async fn download_snapshot(url: &str) -> Result<Vec<u8>, String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(180))
        .user_agent(format!("AnimeOnTrack/{}", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| e.to_string())?;
    let resp = client.get(url).send().await.map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("snapshot download answered HTTP {}", resp.status()));
    }
    if resp.content_length().is_some_and(|n| n as usize > MAX_SNAPSHOT_DOWNLOAD_BYTES) {
        return Err("snapshot is larger than the allowed size".into());
    }
    let bytes = resp.bytes().await.map_err(|e| e.to_string())?;
    if bytes.len() > MAX_SNAPSHOT_DOWNLOAD_BYTES {
        return Err("snapshot is larger than the allowed size".into());
    }
    Ok(bytes.to_vec())
}

/// First-run shortcut: instead of crawling ~21,000 titles from AniList at
/// ~28 requests/minute (over 15 minutes), download the pre-built snapshot (one
/// request), import it in chunks without holding the DB lock for long, and
/// record the full sync as completed so the regular incremental sync only has
/// to close the gap. Best effort: any failure returns `None` and the caller
/// falls back to the normal crawl.
async fn try_snapshot_bootstrap(app: &AppHandle, state: &State<'_, AppState>) -> Option<i64> {
    use crate::anilist::CatalogSyncState;

    let skip = {
        let db = state.db.lock().unwrap();
        let existing = db.catalog_count().unwrap_or(i64::MAX);
        let done = db
            .get_setting("catalog_sync_state")
            .ok()
            .flatten()
            .map(|raw| CatalogSyncState::from_json(&raw).full_sync_completed_at.is_some())
            .unwrap_or(false);
        existing >= BOOTSTRAP_MAX_EXISTING || done
    };
    if skip {
        return None;
    }

    let override_url = std::env::var("AOT_CATALOG_SNAPSHOT_URL").ok().filter(|u| !u.trim().is_empty());
    let url = override_url.clone().unwrap_or_else(|| SNAPSHOT_URL.to_string());
    if override_url.is_none() && !crate::scraper_engine::is_safe_external_url(&url) {
        return None;
    }

    let started = std::time::Instant::now();
    let bytes = match download_snapshot(&url).await {
        Ok(b) => b,
        Err(e) => {
            crate::file_log::warn(&format!("catalog snapshot download failed: {e}"));
            return None;
        }
    };
    let decoded = tauri::async_runtime::spawn_blocking(move || crate::catalog_snapshot::decode(&bytes)).await;
    let (header, items) = match decoded {
        Ok(Ok(v)) => v,
        Ok(Err(e)) => {
            crate::file_log::warn(&format!("catalog snapshot rejected: {e}"));
            return None;
        }
        Err(e) => {
            crate::file_log::warn(&format!("catalog snapshot decode task failed: {e}"));
            return None;
        }
    };

    let total = items.len() as i64;
    let mut inserted = 0i64;
    for (chunk_idx, chunk) in items.chunks(1000).enumerate() {
        {
            let db = state.db.lock().unwrap();
            match db.import_catalog_snapshot(chunk, (chunk_idx * 1000) as i64) {
                Ok(n) => inserted += n as i64,
                Err(e) => {
                    crate::file_log::warn(&format!("catalog snapshot import failed: {e}"));
                    return None;
                }
            }
        }
        let done = ((chunk_idx + 1) * 1000).min(items.len()) as i64;
        let _ = app.emit("catalog-sync-progress", CatalogSyncProgress { synced: done, total });
        // Yield so other commands get the DB lock between chunks.
        tokio::task::yield_now().await;
    }

    let at = chrono::DateTime::from_timestamp(header.generated_at, 0)
        .unwrap_or_else(chrono::Utc::now)
        .to_rfc3339();
    {
        let db = state.db.lock().unwrap();
        let st = CatalogSyncState { completed: Vec::new(), full_sync_completed_at: Some(at) };
        let _ = db.set_setting("catalog_sync_state", &st.to_json());
    }
    crate::file_log::info(&format!(
        "catalog snapshot imported: {inserted} new of {total} rows in {} ms",
        started.elapsed().as_millis()
    ));
    Some(inserted)
}

/// Write a snapshot of the whole local catalog next to the database
/// (`catalog-snapshot.json.gz` in the app data dir) and return its path. This is
/// how the owner produces the file that is published for new installs.
#[tauri::command]
pub async fn export_catalog_snapshot(app: AppHandle, state: State<'_, AppState>) -> Result<String, String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    let items = {
        let db = state.db.lock().unwrap();
        db.catalog_all_for_snapshot().map_err(|e| e.to_string())?
    };
    let path = dir.join("catalog-snapshot.json.gz");
    let out = path.clone();
    tauri::async_runtime::spawn_blocking(move || -> Result<(), String> {
        let bytes = crate::catalog_snapshot::encode(chrono::Utc::now().timestamp(), &items).map_err(|e| e.to_string())?;
        std::fs::write(&out, bytes).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())??;
    Ok(path.to_string_lossy().into_owned())
}

async fn run_catalog_sync(
    app: AppHandle,
    state: State<'_, AppState>,
    force_full: bool,
) -> Result<i64, String> {
    use crate::anilist::{build_partitions, incremental_partitions, pending_partitions, CatalogSyncState};
    use chrono::Datelike;
    use std::sync::atomic::Ordering;

    // One sync at a time: the manual "Sync" button and the auto-incremental
    // startup trigger could otherwise both run `run_catalog_sync`
    // concurrently, interleaving read-modify-write updates to
    // `catalog_sync_state` (one run's completed-partitions write can clobber
    // the other's stale in-memory copy) and doubling AniList's shared
    // ~30 req/min rate-limit consumption. Report the current count rather
    // than erroring the caller — a concurrent trigger isn't a failure, just
    // redundant.
    if state.catalog_sync_running.swap(true, Ordering::SeqCst) {
        let db = state.db.lock().unwrap();
        return db.catalog_count().map_err(|e| e.to_string());
    }
    let _clear = crate::commands::follow::RunningGuard(&state.catalog_sync_running);

    // A new install imports the pre-built snapshot instead of crawling (see
    // `try_snapshot_bootstrap`); on any failure the regular crawl below runs.
    if !force_full {
        let _ = try_snapshot_bootstrap(&app, &state).await;
    }

    const PAGE_SIZE: i64 = 50;
    // Real total (~21,000) can't be known up front (AniList's own totals
    // are fake) — this is a stable baseline for the progress bar's `max`,
    // not a claim of precision. Clamped up if synced ever exceeds it so the
    // bar can't show >100%.
    const FULL_SYNC_ESTIMATE: i64 = 21_000;
    // Only skip the pacing sleep when headroom is still near a fully-fresh
    // window (>= this many of the 30 requests/min still available) — a live
    // run (2026-07-10) with a low burst threshold (skip-sleep whenever
    // remaining >= 3) blew through a whole window in a few seconds of RTT
    // and then stayed rate-limited for the next ~50s of the rolling window
    // regardless of pacing, because AniList's window doesn't refill per
    // request the way a token bucket would — it's closer to a rolling
    // 60s window, so burning most of it in a handful of seconds means the
    // next ~30 requests are blocked no matter how long a *per-request*
    // sleep is, until that whole window ages out. Raising the floor close
    // to the window size means we pace almost every request (~2.1s apart,
    // ~28.6 req/min, safely under the 30/min cap) and only skip sleeping
    // for the handful of requests right after a window resets.
    const RATE_HEADROOM_FLOOR: i64 = 27;
    const PACED_SLEEP: std::time::Duration = std::time::Duration::from_millis(2100);

    let current_year = chrono::Utc::now().year();

    let mut sync_state = if force_full {
        CatalogSyncState::default()
    } else {
        let db = state.db.lock().unwrap();
        db.get_setting("catalog_sync_state")
            .map_err(|e| e.to_string())?
            .map(|raw| CatalogSyncState::from_json(&raw))
            .unwrap_or_default()
    };

    let is_incremental = sync_state.full_sync_completed_at.is_some();
    let partitions_to_run: Vec<_> = if is_incremental {
        incremental_partitions(current_year)
    } else {
        pending_partitions(&build_partitions(current_year), &sync_state.completed)
    };

    let mut synced = {
        let db = state.db.lock().unwrap();
        db.catalog_count().map_err(|e| e.to_string())?
    };

    for partition in &partitions_to_run {
        let mut page = 1i64;
        loop {
            // Up to ten consecutive pages per request (the rate limit counts
            // requests, not rows); never past the partition's own page cap.
            let batch_pages = match partition.max_pages {
                Some(cap) => crate::anilist::PAGES_PER_REQUEST.min(cap - page + 1).max(1),
                None => crate::anilist::PAGES_PER_REQUEST,
            };
            let started = std::time::Instant::now();
            let result = crate::anilist::fetch_partition_pages(partition, page, batch_pages, PAGE_SIZE)
                .await
                .map_err(|e| e.to_string())?;

            {
                let db = state.db.lock().unwrap();
                let items: Vec<_> = result
                    .items
                    .iter()
                    .enumerate()
                    .map(|(idx, anime)| (anime, synced + idx as i64))
                    .collect();
                db.upsert_catalog_anime_batch(&items).map_err(|e| e.to_string())?;
                synced = db.catalog_count().map_err(|e| e.to_string())?;
            }
            let total = FULL_SYNC_ESTIMATE.max(synced);
            let _ = app.emit("catalog-sync-progress", CatalogSyncProgress { synced, total });

            let reached_page_cap = partition.max_pages.is_some_and(|cap| page + batch_pages > cap);
            if !result.has_next_page || reached_page_cap {
                break;
            }
            page += batch_pages;
            let should_pace = result.rate_remaining.map(|r| r < RATE_HEADROOM_FLOOR).unwrap_or(true);
            if should_pace {
                tokio::time::sleep(PACED_SLEEP.saturating_sub(started.elapsed())).await;
            }
        }

        // Only a from-scratch/resuming full sync tracks per-partition
        // completion — incremental syncs always re-run their fixed window
        // and don't consult this list, so there's nothing useful to record.
        if !is_incremental {
            sync_state.completed.push(partition.label.clone());
            let db = state.db.lock().unwrap();
            db.set_setting("catalog_sync_state", &sync_state.to_json())
                .map_err(|e| e.to_string())?;
        }
    }

    if !is_incremental {
        sync_state.full_sync_completed_at = Some(chrono::Utc::now().to_rfc3339());
        let db = state.db.lock().unwrap();
        db.set_setting("catalog_sync_state", &sync_state.to_json())
            .map_err(|e| e.to_string())?;
    }

    let _ = app.emit("catalog-sync-progress", CatalogSyncProgress { synced, total: synced });
    Ok(synced)
}

/// Link the series the local catalog could not resolve by searching AniList
/// (docs/anilist-source-of-truth.md). At most 25 distinct titles per run and at
/// most 2 requests per title (the title, then its parenthetical spelling), paced
/// like the other AniList loops; the DB lock is never held across a request. A
/// request error ends the run without recording a miss, so it is retried next
/// launch; a title nothing confidently matches is remembered for 14 days.
#[tauri::command]
pub async fn link_unlinked_via_anilist(state: State<'_, AppState>) -> Result<i64, String> {
    const PACED_SLEEP: std::time::Duration = std::time::Duration::from_millis(2100);

    let now = chrono::Utc::now().timestamp();
    let targets = {
        let db = state.db.lock().unwrap();
        db.unlinked_for_search(25, now).map_err(|e| e.to_string())?
    };

    let mut linked_count = 0i64;
    'targets: for target in targets {
        let mut picked: Option<crate::anilist::CatalogAnime> = None;
        for query in crate::matching::search_queries(&target.title) {
            let cands = match crate::anilist::search_anime(&query).await {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("[anilist-search] request failed, stopping the run: {e}");
                    break 'targets;
                }
            };
            let hit = crate::matching::pick_search_candidate(&target.title, &cands);
            if let Some(found) = hit.and_then(|id| cands.into_iter().find(|c| c.id == id)) {
                picked = Some(found);
                break;
            }
            tokio::time::sleep(PACED_SLEEP).await;
        }

        {
            let db = state.db.lock().unwrap();
            match &picked {
                Some(anime) => match db.link_search_result(&target.series_ids, anime) {
                    Ok(()) => {
                        eprintln!("[anilist-search] '{}' -> {} ({})", target.title, anime.id, anime.title);
                        linked_count += target.series_ids.len() as i64;
                    }
                    Err(e) => eprintln!("[anilist-search] linking '{}' failed: {e}", target.title),
                },
                None => {
                    let _ = db.record_search_miss(&target.norm_title, now);
                }
            }
        }
        tokio::time::sleep(PACED_SLEEP).await;
    }

    Ok(linked_count)
}

#[cfg(test)]
mod auto_sync_tests {
    use super::{should_auto_sync_catalog, should_stamp_auto_sync};
    use chrono::{Duration, Utc};

    #[test]
    fn never_run_before_should_sync() {
        assert!(should_auto_sync_catalog(None, Utc::now()));
    }

    #[test]
    fn corrupt_timestamp_should_sync() {
        assert!(should_auto_sync_catalog(Some("not a date"), Utc::now()));
    }

    #[test]
    fn recent_run_should_not_sync() {
        let now = Utc::now();
        let last = (now - Duration::hours(1)).to_rfc3339();
        assert!(!should_auto_sync_catalog(Some(&last), now));
    }

    #[test]
    fn run_past_cooldown_should_sync_again() {
        let now = Utc::now();
        let last = (now - Duration::hours(13)).to_rfc3339();
        assert!(should_auto_sync_catalog(Some(&last), now));
    }

    #[test]
    fn exactly_at_cooldown_boundary_should_sync() {
        let now = Utc::now();
        let last = (now - Duration::hours(12)).to_rfc3339();
        assert!(should_auto_sync_catalog(Some(&last), now));
    }

    /// The bug: the timestamp was written *before* the sync ran, so a
    /// transient failure (AniList down, rate-limited, offline at launch) burned
    /// the full 12h cooldown and the next launch skipped instead of retrying.
    /// Walks the two predicates in the same order `maybe_sync_catalog_incremental`
    /// does, over the same persisted string the settings table holds.
    #[test]
    fn a_failed_sync_does_not_burn_the_cooldown_but_a_successful_one_does() {
        let t0 = Utc::now();
        let mut stamp: Option<String> = None; // never auto-synced

        // Launch 1: due, and the sync fails — nothing is stamped.
        assert!(should_auto_sync_catalog(stamp.as_deref(), t0));
        let failed: Result<i64, String> = Err("AniList unreachable".into());
        assert!(!should_stamp_auto_sync(&failed));
        if should_stamp_auto_sync(&failed) {
            stamp = Some(t0.to_rfc3339());
        }

        // Launch 2, a minute later: must retry rather than wait out 12h.
        let t1 = t0 + Duration::minutes(1);
        assert!(
            should_auto_sync_catalog(stamp.as_deref(), t1),
            "a failed sync must not block the next attempt"
        );

        // This time it succeeds — now the cooldown starts.
        let ok: Result<i64, String> = Ok(21_000);
        assert!(should_stamp_auto_sync(&ok));
        if should_stamp_auto_sync(&ok) {
            stamp = Some(t1.to_rfc3339());
        }
        assert!(
            !should_auto_sync_catalog(stamp.as_deref(), t1 + Duration::hours(1)),
            "a successful sync must still hold the cooldown"
        );
        assert!(should_auto_sync_catalog(stamp.as_deref(), t1 + Duration::hours(13)));
    }
}

#[cfg(test)]
mod status_refresh_tests {
    use super::should_refresh_catalog_status;
    use chrono::{Duration, Utc};

    #[test]
    fn never_refreshed_should_run() {
        assert!(should_refresh_catalog_status(None, Utc::now()));
    }

    #[test]
    fn corrupt_timestamp_should_run() {
        assert!(should_refresh_catalog_status(Some("not-a-date"), Utc::now()));
    }

    #[test]
    fn recent_refresh_under_6h_should_skip() {
        let now = Utc::now();
        let last = (now - Duration::hours(3)).to_rfc3339();
        assert!(!should_refresh_catalog_status(Some(&last), now));
    }

    #[test]
    fn past_6h_should_run_again() {
        let now = Utc::now();
        let last = (now - Duration::hours(7)).to_rfc3339();
        assert!(should_refresh_catalog_status(Some(&last), now));
    }

    #[test]
    fn boundary_at_6h_should_run() {
        let now = Utc::now();
        let last = (now - Duration::hours(6)).to_rfc3339();
        assert!(should_refresh_catalog_status(Some(&last), now));
    }

    #[test]
    fn backfill_and_status_guards_turn_away_concurrent_calls() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let running = AtomicBool::new(false);
        assert!(!running.swap(true, Ordering::SeqCst));
        {
            let _guard = crate::commands::follow::RunningGuard(&running);
            assert!(running.swap(true, Ordering::SeqCst));
        }
        assert!(!running.load(Ordering::SeqCst));
    }
}
