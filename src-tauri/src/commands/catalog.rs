use super::*;

/// Full genre list for one series, for the Listas view's "Quiero ver" rows.
#[tauri::command]
pub fn get_series_genres(state: State<'_, AppState>, series_id: i64) -> Result<Vec<String>, String> {
    let db = state.db.lock().unwrap();
    db.list_series_genres(series_id).map_err(|e| e.to_string())
}

/// This user's top `limit` genres by affinity score (positive only), for
/// the "Tus géneros favoritos" chip row in the swipe UI — makes the
/// otherwise-invisible taste-weighting in `discover_swipe_card` visible.
#[tauri::command]
pub fn get_top_genres(state: State<'_, AppState>, limit: usize) -> Result<Vec<GenreAffinity>, String> {
    let src = get_source_id(&state)?;
    let db = state.db.lock().unwrap();
    let mut scored: Vec<(String, f64)> = db
        .get_genre_affinity(src)
        .map_err(|e| e.to_string())?
        .into_iter()
        .filter(|(_, score)| *score > 0.0)
        .collect();
    // Tie-break alphabetically: scores come out of a HashMap, so without a
    // secondary key equal-scored genres would reorder from call to call.
    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal).then_with(|| a.0.cmp(&b.0)));
    scored.truncate(limit);
    Ok(scored.into_iter().map(|(genre, score)| GenreAffinity { genre, score }).collect())
}

/// Full matched AniList catalog entry for a series (cover/genres/format/
/// episodes/score/studio/url) — SeriesDetail's info panel source now that
/// anime metadata comes from AniList rather than the scraped site. `None`
/// when nothing in the synced catalog matches.
#[tauri::command]
pub fn get_catalog_info_for_series(
    state: State<'_, AppState>,
    series_id: i64,
) -> Result<Option<crate::anilist::CatalogAnime>, String> {
    let db = state.db.lock().unwrap();
    db.catalog_info_for_series(series_id).map_err(|e| e.to_string())
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
pub fn get_anime_catalog(
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
pub fn get_catalog_facets(state: State<'_, AppState>) -> Result<CatalogFacets, String> {
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
    {
        let db = state.db.lock().unwrap();
        db.set_setting("catalog_auto_sync_last_at", &chrono::Utc::now().to_rfc3339())
            .map_err(|e| e.to_string())?;
    }
    run_catalog_sync(app, state, false).await.map(Some)
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

#[derive(Serialize, Clone)]
struct CatalogBackfillProgress {
    done: i64,
    total: i64,
}

/// Re-fetch catalog rows stored before `title_romaji`/`duration`/`status`/
/// `studio`/`start_date` were part of the sync query, filling those columns in
/// place.
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

    for batch in stale_ids.chunks(crate::anilist::MAX_IDS_PER_REQUEST) {
        let fetched = crate::anilist::fetch_by_ids(batch).await.map_err(|e| e.to_string())?;
        {
            let db = state.db.lock().unwrap();
            for anime in &fetched {
                let sort_order = db.catalog_sort_order(anime.id).map_err(|e| e.to_string())?;
                db.upsert_catalog_anime(anime, sort_order).map_err(|e| e.to_string())?;
            }
        }
        // Counted over the batch, not the response: ids AniList no longer
        // serves (deleted/merged entries) never come back, and counting only
        // what returned would leave the progress bar permanently short of its
        // total and re-request those same dead ids on every future run.
        done += batch.len() as i64;
        let _ = app.emit("catalog-backfill-progress", CatalogBackfillProgress { done, total });
        tokio::time::sleep(PACED_SLEEP).await;
    }
    Ok(done)
}

async fn run_catalog_sync(
    app: AppHandle,
    state: State<'_, AppState>,
    force_full: bool,
) -> Result<i64, String> {
    use crate::anilist::{build_partitions, incremental_partitions, pending_partitions, CatalogSyncState};
    use chrono::Datelike;

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
            let result = crate::anilist::fetch_partition_page(partition, page, PAGE_SIZE)
                .await
                .map_err(|e| e.to_string())?;

            {
                let db = state.db.lock().unwrap();
                for (idx, anime) in result.items.iter().enumerate() {
                    let sort_order = synced + idx as i64;
                    db.upsert_catalog_anime(anime, sort_order).map_err(|e| e.to_string())?;
                }
                synced = db.catalog_count().map_err(|e| e.to_string())?;
            }
            let total = FULL_SYNC_ESTIMATE.max(synced);
            let _ = app.emit("catalog-sync-progress", CatalogSyncProgress { synced, total });

            let reached_page_cap = partition.max_pages.is_some_and(|cap| page >= cap);
            if !result.has_next_page || reached_page_cap {
                break;
            }
            page += 1;
            let should_pace = result.rate_remaining.map(|r| r < RATE_HEADROOM_FLOOR).unwrap_or(true);
            if should_pace {
                tokio::time::sleep(PACED_SLEEP).await;
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

#[cfg(test)]
mod auto_sync_tests {
    use super::should_auto_sync_catalog;
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
}
