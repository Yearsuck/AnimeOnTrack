use super::*;

/// Prefix for the per-site cached genre-archive list
/// (`genre_list:{site_id}`) — see `ensure_genre_list`.
const GENRE_LIST_KEY_PREFIX: &str = "genre_list";

pub fn load_genre_list(db: &Db, site_id: &str) -> Result<Vec<(String, String)>, String> {
    let key = format!("{GENRE_LIST_KEY_PREFIX}:{site_id}");
    let raw = db.get_setting(&key).map_err(|e| e.to_string())?;
    match raw {
        Some(s) => serde_json::from_str(&s).map_err(|e| e.to_string()),
        None => Ok(Vec::new()),
    }
}

pub fn save_genre_list(db: &Db, site_id: &str, list: &[(String, String)]) -> Result<(), String> {
    let key = format!("{GENRE_LIST_KEY_PREFIX}:{site_id}");
    let raw = serde_json::to_string(list).map_err(|e| e.to_string())?;
    db.set_setting(&key, &raw).map_err(|e| e.to_string())
}

/// Cached genre (slug, name) list, scraped once per install (per site) and
/// reused after (mirrors the `mirror_urls` settings-cache pattern already
/// used elsewhere). A site with no genre archive (`genre_list_url` returning
/// `""`, the trait default) always scrapes an empty list, caches it, and
/// `discover_swipe_card`'s caller sees `Ok(vec![])` — same "no genres" error
/// path as an actual scrape failure would hit.
pub async fn ensure_genre_list(
    app: &AppHandle,
    db_mirrors: &[String],
    state: &State<'_, AppState>,
    a: &dyn SiteAdapter,
    site_id: &str,
) -> Result<Vec<(String, String)>, String> {
    let cached = {
        let db = state.db.lock().unwrap();
        load_genre_list(&db, site_id)?
    };
    if !cached.is_empty() {
        return Ok(cached);
    }
    let (_scraped, pairs, _mirror) = scrape_via_mirrors(app, db_mirrors, &a.genre_list_url(""), true, |scraped| {
        a.parse_genre_list(&scraped.html)
    })
    .await?;
    let db = state.db.lock().unwrap();
    save_genre_list(&db, site_id, &pairs)?;
    Ok(pairs)
}

pub fn path_of(url: &str) -> Result<String, String> {
    let u = url::Url::parse(url).map_err(|_| format!("url inválida: {url}"))?;
    Ok(format!("{}{}", u.path(), u.query().map(|q| format!("?{q}")).unwrap_or_default()))
}

/// Fetch and parse a series detail page, falling through mirrors the same
/// way every other scrape does. An empty genre list is treated the same as
/// "page loaded but didn't parse" (see `scrape_via_mirrors`'s doc comment) —
/// it means this mirror's detail-page markup didn't match, not that the
/// series genuinely has zero genres.
pub async fn fetch_series_detail(
    app: &AppHandle,
    mirrors: &[String],
    series_url: &str,
    a: &dyn SiteAdapter,
) -> Result<SeriesDetail, String> {
    let path = path_of(series_url)?;
    let (_scraped, details, _mirror) = scrape_via_mirrors(app, mirrors, &path, true, |scraped| {
        let d = a.parse_series_detail(&scraped.html)?;
        if d.genres.is_empty() {
            Err(anyhow::anyhow!("no genres parsed (likely wrong/incompatible mirror)"))
        } else {
            Ok(vec![d])
        }
    })
    .await?;
    details.into_iter().next().ok_or_else(|| "empty series detail page".to_string())
}

/// Backfill one series' genres/kind if it has no `series_genres` rows yet.
/// Returns true if a fetch was attempted (regardless of success) — used by
/// the caller to decide whether to apply the polite inter-series delay.
/// Locks `db` only for the short sync checks/writes, never across the
/// network `.await`, same discipline `refresh()`'s loop already follows.
pub async fn backfill_series_genre_if_missing(
    app: &AppHandle,
    db: &Mutex<Db>,
    mirrors: &[String],
    series: &Series,
    a: &dyn SiteAdapter,
) -> bool {
    let needs = {
        let db = db.lock().unwrap();
        db.series_needs_genre_backfill(series.id).unwrap_or(false)
    };
    if !needs {
        return false;
    }
    if let Ok(detail) = fetch_series_detail(app, mirrors, &series.url, a).await {
        let db = db.lock().unwrap();
        let _ = db.insert_series_genres(series.id, &detail.genres);
        if let Some(kind) = &detail.kind {
            let _ = db.set_kind(series.id, kind);
        }
    }
    true
}

pub async fn fetch_episode_list_for(
    app: &AppHandle,
    mirrors: &[String],
    series_url: &str,
    a: &dyn SiteAdapter,
) -> Result<Vec<Episode>, String> {
    let path = path_of(series_url)?;
    let (_scraped, eps, _mirror) = scrape_via_mirrors_with_script(
        app,
        mirrors,
        &path,
        true,
        a.episode_fetch_script(),
        |scraped| a.parse_series(scraped.extra.as_deref().unwrap_or(&scraped.html)),
    )
    .await?;
    Ok(eps)
}

pub fn slug_from_url(url: &str) -> String {
    url.trim_end_matches('/').rsplit('/').next().unwrap_or("").to_string()
}

async fn scan_airing_via_mirrors(
    app: &AppHandle,
    state: &State<'_, AppState>,
    mirrors: Vec<String>,
    a: &dyn SiteAdapter,
    site_id: &str,
) -> Result<Vec<Series>, String> {
    emit_refresh_progress(app, 0, 1, "Escaneando listado de estrenos");
    // Walk every page of the airing listing, not just the first. Some sites
    // paginate their "en emisión" directory (TioAnime: ~5 pages of 20), so a
    // single-page scan silently dropped every followed show past page 1 from
    // the airing grid. Page 1 must succeed (real content) and picks the working
    // mirror; later pages are fetched from that mirror and an empty/failed page
    // is the natural end of pagination, not a scan failure. `airing_page_url`'s
    // default is a single page, so non-paginating sites behave exactly as before.
    const MAX_AIRING_PAGES: u32 = 25;
    let page1_path = a.airing_page_url("", 1).unwrap_or_else(|| a.airing_url(""));
    let (_scraped, mut series, working_mirror) =
        scrape_via_mirrors(app, &mirrors, &page1_path, false, |scraped| a.parse_airing(&scraped.html)).await?;
    let mut seen_slugs: std::collections::HashSet<String> =
        series.iter().map(|s| s.slug.clone()).collect();
    let page_mirrors = vec![working_mirror.clone()];
    for page in 2..=MAX_AIRING_PAGES {
        let Some(path) = a.airing_page_url("", page) else { break };
        emit_refresh_progress(app, 0, 1, &format!("Escaneando estrenos (página {page})"));
        // Wrap the parse in a one-element Vec so scrape_via_mirrors doesn't read
        // an empty page as a mirror failure (same trick as search_site): an
        // empty page is the end of pagination, handled right below.
        let fetched = scrape_via_mirrors(app, &page_mirrors, &path, false, |scraped| {
            a.parse_airing(&scraped.html).map(|v| vec![v])
        })
        .await;
        let Ok((_s, mut pages, _m)) = fetched else { break };
        let Some(page_series) = pages.pop() else { break };
        if page_series.is_empty() {
            break;
        }
        let mut added = 0;
        for s in page_series {
            if seen_slugs.insert(s.slug.clone()) {
                series.push(s);
                added += 1;
            }
        }
        // A page with no *new* series (a site that clamps `p` to the last page
        // and re-serves it) also ends the walk — avoids looping to the cap.
        if added == 0 {
            break;
        }
    }
    emit_refresh_progress(app, 1, 1, "Listado completo");
    // Cover images are intentionally NOT fetched here: doing it for every
    // series on the airing list (~150 at once) reads as scraping abuse to
    // Cloudflare and gets rate-limited regardless of session validity. Covers
    // are fetched one at a time in `refresh`, only for followed series.

    let site_name = adapter::all_sites()
        .iter()
        .find(|s| s.id == site_id)
        .map(|s| s.name)
        .unwrap_or(site_id);

    let db = state.db.lock().unwrap();
    save_mirrors(&db, site_id, &mirrors)?;
    let src = db
        .upsert_source(site_name, &working_mirror, site_id)
        .map_err(|e| e.to_string())?;
    let mut upserted_ids: Vec<i64> = Vec::with_capacity(series.len());
    for s in &series {
        upserted_ids.push(db.upsert_series(src, s).map_err(|e| e.to_string())?);
    }
    // Cross-site follow carry-over: a series followed on ANOTHER site that
    // matches (by title) one just scanned here inherits the follow + a
    // progress watermark, so switching sites doesn't strand your library. The
    // watermark is applied later, once refresh() fetches this site's episode
    // list (see refresh + db::carry_follow). Matching is title-based and
    // conservative (matching::MATCH_THRESHOLD) — a wrong carry would falsely
    // follow + mark-seen the wrong show.
    let followed_elsewhere = db
        .followed_titles_with_watermark(src)
        .map_err(|e| e.to_string())?;
    if !followed_elsewhere.is_empty() {
        for (idx, watermark) in plan_carryover(&series, &followed_elsewhere) {
            db.carry_follow(upserted_ids[idx], watermark).map_err(|e| e.to_string())?;
        }
    }
    // Keep completion consistent across sites right after a switch/scan: a
    // show completed elsewhere shouldn't resurface as unwatched-pending here.
    db.reconcile_completion_across_sites().map_err(|e| e.to_string())?;
    // And ordinary per-episode progress too — carry_follow above only ever
    // fires once (the first time a site gains the follow); a show followed
    // on two sites for a while needs its watermark re-checked on every
    // switch/scan, not just at first-follow, or episodes watched on the
    // other site keep reappearing as pending here.
    db.sync_seen_progress_across_sites().map_err(|e| e.to_string())?;
    // A site's own scrape never un-airs a show once scraped as airing (see
    // sync_finished_status_from_catalog's doc comment) — AniList's synced
    // status is the only signal that actually catches a real finish.
    db.sync_finished_status_from_catalog().map_err(|e| e.to_string())?;

    *state.source_id.lock().unwrap() = Some(src);
    let airing = db.list_airing(src).map_err(|e| e.to_string())?;
    drop(db);

    // Auto-import the whole library onto this site on every airing (re)scan —
    // the airing-page carry-over above only reaches the ~airing overlap, so
    // finished follows from other sites still need the paced per-series search
    // that `spawn_library_import` runs in the background. Fire-and-forget: the
    // airing list returns now; the import (a no-op once the library is fully
    // resolved) runs behind it, guarded against overlapping scans.
    spawn_library_import(app.clone());
    // Same reasoning, for episodes: a show carried over (or already followed)
    // on this site may never have had its episode list actually fetched if
    // this site only just became active — refresh() is the only place that
    // normally happens, and it only runs at app startup or on a manual
    // "Actualizar" against whichever site is active then. Fire-and-forget,
    // paced, guarded against overlap — see `spawn_episode_backfill`.
    spawn_episode_backfill(app.clone());
    Ok(airing)
}

/// First-run scan: seed the mirror list with `base_url` (kept first if new),
/// then scan the airing list trying every configured mirror in order. Always
/// scans the currently-active site (`state.active_site_id`) — after a
/// Settings site switch, that's already been updated before this is called.
#[tauri::command]
pub async fn scan_airing(
    app: AppHandle,
    state: State<'_, AppState>,
    base_url: String,
) -> Result<Vec<Series>, String> {
    let site_id = get_active_site_id(&state);
    let a = get_active_adapter(&state)?;
    let existing = {
        let db = state.db.lock().unwrap();
        load_mirrors(&db, &site_id)?
    };
    let mirrors = with_mirror(existing, &base_url);
    scan_airing_via_mirrors(&app, &state, mirrors, a.as_ref(), &site_id).await
}

/// Re-scan the airing list using only the mirrors already configured in
/// Settings for the active site (no new URL supplied).
#[tauri::command]
pub async fn rescan_airing(app: AppHandle, state: State<'_, AppState>) -> Result<Vec<Series>, String> {
    let site_id = get_active_site_id(&state);
    let a = get_active_adapter(&state)?;
    let mirrors = {
        let db = state.db.lock().unwrap();
        load_mirrors(&db, &site_id)?
    };
    scan_airing_via_mirrors(&app, &state, mirrors, a.as_ref(), &site_id).await
}

#[tauri::command]
pub fn list_airing(state: State<'_, AppState>) -> Result<Vec<Series>, String> {
    let src = get_source_id(&state)?;
    let db = state.db.lock().unwrap();
    db.list_airing(src).map_err(|e| e.to_string())
}

/// Same series/order as `list_airing`, each paired with a real premiere date
/// (when known) for the frontend's "Esta temporada" filter. See
/// docs/superpowers/specs/2026-07-13-airing-this-season-design.md and its
/// 2026-07 addendum: a scraped first episode (`db::first_episode_dates`) is
/// preferred when present — day-accurate, from the actual episode — with a
/// fallback to the locally-synced AniList catalog's `start_date`
/// (`db::catalog_start_dates_by_normalized_title`) so unfollowed/unopened
/// airing series (which have no scraped episode data at all) still resolve a
/// verdict instead of being silently excluded. The catalog fallback requires
/// a Catálogo sync to have populated `start_date`; still `None` when neither
/// source has a date (title not found in the synced catalog, or synced
/// before this field existed).
#[tauri::command]
pub fn list_airing_season(state: State<'_, AppState>) -> Result<Vec<AiringItem>, String> {
    let src = get_source_id(&state)?;
    let db = state.db.lock().unwrap();
    let series = db.list_airing(src).map_err(|e| e.to_string())?;
    let first_dates = db.first_episode_dates(src).map_err(|e| e.to_string())?;
    let catalog_dates = db.catalog_start_dates_by_normalized_title().map_err(|e| e.to_string())?;
    Ok(series
        .into_iter()
        .map(|s| {
            let scraped = first_dates
                .get(&s.id)
                .and_then(|raw| parse_spanish_date(raw))
                .and_then(|d| d.and_hms_opt(0, 0, 0))
                .map(|dt| dt.and_utc().timestamp());
            let first_episode_at = scraped
                .or_else(|| catalog_dates.get(&crate::matching::normalize_title(&s.title)).copied());
            AiringItem { series: s, first_episode_at }
        })
        .collect())
}

/// Backfill genres/kind for every followed series still missing
/// `series_genres` rows (not just the one-per-refresh-cycle `refresh()`
/// does). Same politeness delay between series as `refresh()`. Returns the
/// count of series that actually gained genres.
#[tauri::command]
pub async fn backfill_genres(app: AppHandle, state: State<'_, AppState>) -> Result<i64, String> {
    let src = get_source_id(&state)?;
    let site_id = get_active_site_id(&state);
    let a = get_active_adapter(&state)?;
    let (candidates, mirrors) = {
        let db = state.db.lock().unwrap();
        let followed = db.list_followed(src).map_err(|e| e.to_string())?;
        let mut candidates = Vec::new();
        for s in followed {
            if db.series_needs_genre_backfill(s.id).map_err(|e| e.to_string())? {
                candidates.push(s);
            }
        }
        (candidates, load_mirrors(&db, &site_id)?)
    };
    let total = candidates.len();
    let mut filled = 0i64;
    for (idx, s) in candidates.into_iter().enumerate() {
        emit_refresh_progress(&app, idx, total, &s.title);
        if backfill_series_genre_if_missing(&app, &state.db, &mirrors, &s, a.as_ref()).await {
            let got = {
                let db = state.db.lock().unwrap();
                !db.series_needs_genre_backfill(s.id).map_err(|e| e.to_string())?
            };
            if got {
                filled += 1;
            }
            tokio::time::sleep(std::time::Duration::from_millis(800)).await;
        }
    }
    emit_refresh_progress(&app, total, total, "Completado");
    Ok(filled)
}

/// How many series' episode-list pages to fetch concurrently in refresh().
/// Cover images stay strictly one-at-a-time regardless (see the cover-fetch
/// comment below) — this only parallelizes the plain HTML episode-list
/// fetch, not the thing CLAUDE.md specifically calls out as abuse-prone.
///
/// Kept equal to `scraper_engine::SCRAPE_CONCURRENCY` (raised together to 4
/// on 2026-08-14): the semaphore there is the real ceiling, so a larger
/// value here would just queue behind it, and a smaller one would leave
/// permits unused. The `chunk` match below has one `tokio::join!` arm per
/// size up to this value — raising it further needs a matching arm.
const REFRESH_CONCURRENCY: usize = 4;

/// How long a followed series that is *absent* from the airing listing goes
/// between episode-list rechecks. The spec drafted this as 7 days on the
/// assumption "absent from the listing = finished", but live verification
/// (2026-07-10) disproved that: the schedule listing only carries ~77 series
/// while 114 are marked airing, and two followed shows absent from it
/// (Tensei Slime T4, Ryoumin 0-nin) got real new episodes the same day. So
/// absent series are rechecked daily — new episodes there arrive at most
/// ~24h late instead of ~7 days, at the cost of one full off-listing sweep
/// per day (first refresh of the day), with every later refresh that day
/// still hitting the 1-fetch fast path.
const OFF_LISTING_RECHECK_SECS: i64 = 24 * 3600;

/// On-listing followed series whose airing card shows a non-numeric "??"
/// badge give no count signal at all; the future countdown can't rule out a
/// fresh episode either (live-verified: it's never observed in the past,
/// since the site rolls it forward the instant an episode posts), so an
/// unknown badge used to mean an indefinite skip (Bug A, 2026-07-12
/// airing-refresh-missing-episodes fix — proven live against "One Piece:
/// Arco de Elbaph", permanently stuck until a force refresh). Re-fetch at
/// most this often instead. Cheap: only a handful of "??" series were ever
/// observed live (<=7), at most once per 6h each.
const UNKNOWN_BADGE_RECHECK_SECS: i64 = 6 * 3600;

/// The skip decision at the heart of refresh()'s optimization A (see
/// docs/superpowers/specs/2026-07-10-scraper-performance-design.md): given a
/// followed series' fresh airing-listing metadata, does its episode-list
/// page need fetching this cycle? Pure so it can be unit-tested without a
/// scraper or DB — a bug here silently stops detecting new episodes, which
/// is the app's entire purpose.
///
/// Live-verified semantics of the card metadata (2026-07-10, real site):
/// the `.sb` badge is the **upcoming episode's number**, not the count of
/// posted episodes — 8/8 followed series that were provably up to date
/// (fetched moments earlier, no new episodes) all showed `.sb == db+1`, and
/// the fixture agrees (Liar Game: `.sb`=14, newest posted episode 13). And
/// no live card (0/77) carried a past `data-rlsdt`; just-aired series
/// either roll to next week's timestamp or leave the listing entirely.
///
/// Rules, in order:
/// - `force` (the Settings "Forzar recomprobación completa" escape hatch)
///   always fetches.
/// - `listing_scanned == false` (the airing-listing scan itself failed, so
///   there is no fresh metadata) always fetches — behave exactly like the
///   pre-skip-logic refresh rather than trusting stale signals.
/// - Off the listing: fetch only when never checked or the last check is
///   older than `OFF_LISTING_RECHECK_SECS` (see its comment — absent does
///   NOT mean finished on this site).
/// - On the listing with a past countdown: the episode aired and the card
///   hasn't rolled over — fetch, whatever the badge says.
/// - Badge present: fetch iff `badge > db_max+1` (site has posted something
///   we don't have; `db_max` is the highest real episode NUMBER, not a row
///   count — see `Db::max_episode_number`'s doc for why a row count is
///   wrong). `db_max+1` (up to date) and `<= db_max` (our numbering equal or
///   ahead) both skip. Deliberately *not* the spec's literal "badge == db
///   skips": that rule assumed badge = posted count, which the live data
///   disproves — under it, every current weekly series re-fetches forever.
/// - Badge unknown (`"??"`): no count signal at all, and the countdown can't
///   substitute for one (it's never observed in the past — see
///   `UNKNOWN_BADGE_RECHECK_SECS`'s doc), so fall back to a bounded recheck
///   instead of skipping indefinitely (Bug A, 2026-07-12 fix).
#[allow(clippy::too_many_arguments)]
fn should_fetch_series(
    force: bool,
    listing_scanned: bool,
    on_listing: bool,
    next_episode_at: Option<i64>,
    site_episode_count: Option<i64>,
    db_max_episode_number: i64,
    last_checked_age_secs: Option<i64>,
    now_unix: i64,
) -> bool {
    if force || !listing_scanned {
        return true;
    }
    if !on_listing {
        return match last_checked_age_secs {
            None => true,
            Some(age) => age >= OFF_LISTING_RECHECK_SECS,
        };
    }
    if next_episode_at.is_some_and(|t| t <= now_unix) {
        return true;
    }
    match site_episode_count {
        Some(next_number) => next_number > db_max_episode_number + 1,
        None => match last_checked_age_secs {
            None => true,
            Some(age) => age >= UNKNOWN_BADGE_RECHECK_SECS,
        },
    }
}

/// Fetch one series' episode list across mirrors. `None` means "skip, keep
/// cached data" (malformed stored URL, or every mirror failed/mismatched) —
/// same not-an-error semantics `refresh()`'s loop always had here.
async fn fetch_series_episodes(
    app: &AppHandle,
    mirrors: &[String],
    a: &dyn SiteAdapter,
    series_url: &str,
) -> Option<(Vec<Episode>, String, String)> {
    let path = match url::Url::parse(series_url) {
        Ok(u) => format!("{}{}", u.path(), u.query().map(|q| format!("?{q}")).unwrap_or_default()),
        Err(_) => return None,
    };
    match scrape_via_mirrors_with_script(app, mirrors, &path, false, a.episode_fetch_script(), |scraped| {
        a.parse_series(scraped.extra.as_deref().unwrap_or(&scraped.html))
    })
    .await
    {
        Ok((_scraped, eps, working_mirror)) => Some((eps, working_mirror, path)),
        Err(_) => None,
    }
}

/// Fire the site catch-up in the background after a site switch/scan: first
/// catalog-link + finished-status correction (local-only, no network — see
/// `run_episode_backfill`'s inner comment for why this must NOT run
/// synchronously on the calling command), then the episode backfill so
/// followed shows with zero episodes on this site (see
/// `Db::followed_series_without_episodes`'s doc comment for why that
/// happens — mainly a site that became active mid-session) catch up
/// automatically instead of sitting empty until the user happens to have
/// this site active at app startup or clicks "Actualizar". Same
/// fire-and-forget, guarded-against-overlap shape as `spawn_library_import`.
pub fn spawn_episode_backfill(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        if let Err(e) = run_episode_backfill(app).await {
            eprintln!("[scrape] episode backfill after site switch failed: {e}");
        }
    });
}

async fn run_episode_backfill(app: AppHandle) -> Result<(), String> {
    use std::sync::atomic::Ordering;
    use tauri::Manager;
    const PACED: std::time::Duration = std::time::Duration::from_millis(1500);

    let state = app.state::<AppState>();
    // Only one backfill at a time — same reasoning as library_import_running:
    // an overlapping run would double the Cloudflare-facing request rate.
    if state.episode_backfill_running.swap(true, Ordering::SeqCst) {
        return Ok(());
    }
    let _clear = crate::commands::follow::RunningGuard(&state.episode_backfill_running);

    let (a, mirrors, needing) = {
        let db = state.db.lock().unwrap();
        let src = get_source_id(&state)?;
        let a = get_active_adapter(&state)?;
        let site_id = get_active_site_id(&state);
        let mirrors = load_mirrors(&db, &site_id)?;
        // Catalog linking + the finished-status correction it feeds both
        // moved here (off the synchronous scan_airing_via_mirrors path):
        // series_needing_catalog_link's `OR s.is_airing=1` clause matches
        // nearly every scraped row across all 6 sites (is_airing defaults to
        // true and almost nothing ever flips it — the exact gap
        // sync_finished_status_from_catalog exists to close), thousands of
        // rows, not the "small, targeted" set its own doc comment assumes.
        // Running that synchronously on every site switch blocked the async
        // runtime long enough to crash the app — confirmed live 2026-08-14.
        // Local-only (no network) so it's safe to run before the paced
        // network loop below, not after.
        db.link_series_to_catalog().map_err(|e| e.to_string())?;
        db.sync_finished_status_from_catalog().map_err(|e| e.to_string())?;
        let needing = db.followed_series_without_episodes(src).map_err(|e| e.to_string())?;
        (a, mirrors, needing)
    };

    for s in needing {
        let Some((eps, working_mirror, path)) =
            fetch_series_episodes(&app, &mirrors, a.as_ref(), &s.url).await
        else {
            continue;
        };
        {
            let db = state.db.lock().unwrap();
            let new_url = format!("{working_mirror}{path}");
            if new_url != s.url {
                db.update_series_url(s.id, &new_url).map_err(|e| e.to_string())?;
            }
            // A never-fetched series has no known episode numbers yet, so
            // this is always the full list — same insert path `refresh()`
            // uses, so a later real refresh() sees these as already-known
            // and doesn't duplicate them.
            let known = db.existing_episode_numbers(s.id).map_err(|e| e.to_string())?;
            for mut e in new_episodes(&eps, &known) {
                e.series_id = s.id;
                db.insert_episode(&e).map_err(|e| e.to_string())?;
            }
            db.set_last_checked_at(s.id).map_err(|e| e.to_string())?;
            if let Some(n) = db.take_carried_seen_number(s.id).map_err(|e| e.to_string())? {
                db.set_seen_cascade(s.id, &n.to_string(), true).map_err(|e| e.to_string())?;
            }
        }
        tokio::time::sleep(PACED).await;
    }
    Ok(())
}

/// For each followed series: decide (from one fresh airing-listing fetch)
/// whether its episode page can even have changed, and scrape only the ones
/// that can (falling back across mirrors), inserting new episodes. Returns
/// count of new episodes.
///
/// Optimization A of the scraper-performance design: the airing listing
/// already carries, for every series on it, the next-release timestamp
/// (`data-rlsdt`) and the site's episode count (`.sb`), so a refresh with no
/// new episodes anywhere needs 1 page fetch instead of ~119. See
/// `should_fetch_series` for the exact skip rules and their correctness
/// reasoning. `force: true` (the "Forzar recomprobación completa" Settings
/// action) ignores every skip rule — the user's escape hatch, and the
/// A/B-verification path for the skip logic itself.
#[tauri::command]
pub async fn refresh(app: AppHandle, state: State<'_, AppState>, force: bool) -> Result<i64, String> {
    let refresh_started = std::time::Instant::now();
    let src = get_source_id(&state)?;
    let site_id = get_active_site_id(&state);
    let a = get_active_adapter(&state)?;
    let mirrors = {
        let db = state.db.lock().unwrap();
        load_mirrors(&db, &site_id)?
    };

    // One fresh airing-listing fetch up front — the skip decisions below are
    // only sound against metadata from *this* scan, never a stale one, so
    // this scan deliberately doesn't consult any cache. On failure (all
    // mirrors down) fall back to fetching every followed series, exactly
    // like the pre-skip-logic refresh.
    emit_refresh_progress(&app, 0, 1, "Escaneando listado de estrenos");
    let listing_path = a.airing_url("").to_string();
    let listing_slugs: Option<std::collections::HashSet<String>> =
        match scrape_via_mirrors(&app, &mirrors, &listing_path, false, |scraped| a.parse_airing(&scraped.html)).await {
            Ok((_scraped, series, _mirror)) => {
                let db = state.db.lock().unwrap();
                for s in &series {
                    db.upsert_series(src, s).map_err(|e| e.to_string())?;
                }
                Some(series.into_iter().map(|s| s.slug).collect())
            }
            Err(e) => {
                eprintln!("[scrape] refresh: airing-listing scan failed ({e}); falling back to fetching every followed series");
                None
            }
        };

    // Re-read followed AFTER the listing upserts so next_episode_at /
    // site_episode_count are this scan's values, not last week's.
    let followed = {
        let db = state.db.lock().unwrap();
        // Local-only, no network: catch up any followed show whose watermark
        // on this site fell behind progress made on another (same reason as
        // scan_airing_via_mirrors — see sync_seen_progress_across_sites).
        db.sync_seen_progress_across_sites().map_err(|e| e.to_string())?;
        db.sync_finished_status_from_catalog().map_err(|e| e.to_string())?;
        db.list_followed(src).map_err(|e| e.to_string())?
    };
    let total_series = followed.len();
    let now_unix = chrono::Utc::now().timestamp();

    // Partition into skip/fetch. Skipped series are reported to the progress
    // bar immediately (the bar visibly races through them) so X/Y still
    // covers ALL followed series, not just the fetched ones.
    let mut to_fetch: Vec<Series> = Vec::new();
    let mut idx = 0usize;
    for s in followed {
        let (db_max_number, checked_age) = {
            let db = state.db.lock().unwrap();
            (
                db.max_episode_number(s.id).map_err(|e| e.to_string())?,
                db.last_checked_age_secs(s.id).map_err(|e| e.to_string())?,
            )
        };
        let on_listing = listing_slugs.as_ref().is_some_and(|set| set.contains(&s.slug));
        if should_fetch_series(
            force,
            listing_slugs.is_some(),
            on_listing,
            s.next_episode_at,
            s.site_episode_count,
            db_max_number,
            checked_age,
            now_unix,
        ) {
            to_fetch.push(s);
        } else {
            idx += 1;
            emit_refresh_progress(&app, idx, total_series, &s.title);
        }
    }
    let skipped = total_series - to_fetch.len();
    eprintln!(
        "[scrape] refresh: {skipped} skipped, {} to fetch (force={force}, listing_scanned={})",
        to_fetch.len(),
        listing_slugs.is_some()
    );

    let mut total_new = 0i64;
    for chunk in to_fetch.chunks(REFRESH_CONCURRENCY) {
        for s in chunk {
            emit_refresh_progress(&app, idx, total_series, &s.title);
            idx += 1;
        }

        // Fetch this chunk's episode-list pages concurrently (plain HTML,
        // not the image-bulk case CLAUDE.md warns about) — everything after
        // this (DB writes, cover fetch, genre backfill) stays sequential
        // per series below, so cover images never overlap.
        // `tokio::join!` is fixed-arity, hence one arm per chunk size up to
        // REFRESH_CONCURRENCY; the trailing arm is the sequential fallback
        // for a final short chunk (and would also cover a raised ceiling
        // correctly, just without the parallelism).
        let fetched: Vec<Option<(Vec<Episode>, String, String)>> = match chunk {
            [s0, s1, s2, s3] => {
                let (r0, r1, r2, r3) = tokio::join!(
                    fetch_series_episodes(&app, &mirrors, a.as_ref(), &s0.url),
                    fetch_series_episodes(&app, &mirrors, a.as_ref(), &s1.url),
                    fetch_series_episodes(&app, &mirrors, a.as_ref(), &s2.url),
                    fetch_series_episodes(&app, &mirrors, a.as_ref(), &s3.url),
                );
                vec![r0, r1, r2, r3]
            }
            [s0, s1, s2] => {
                let (r0, r1, r2) = tokio::join!(
                    fetch_series_episodes(&app, &mirrors, a.as_ref(), &s0.url),
                    fetch_series_episodes(&app, &mirrors, a.as_ref(), &s1.url),
                    fetch_series_episodes(&app, &mirrors, a.as_ref(), &s2.url),
                );
                vec![r0, r1, r2]
            }
            [s0, s1] => {
                let (r0, r1) = tokio::join!(
                    fetch_series_episodes(&app, &mirrors, a.as_ref(), &s0.url),
                    fetch_series_episodes(&app, &mirrors, a.as_ref(), &s1.url),
                );
                vec![r0, r1]
            }
            _ => {
                let mut v = Vec::with_capacity(chunk.len());
                for s in chunk {
                    v.push(fetch_series_episodes(&app, &mirrors, a.as_ref(), &s.url).await);
                }
                v
            }
        };

        for (s, result) in chunk.iter().zip(fetched) {
            let Some((eps, working_mirror, path)) = result else { continue };
            {
                let db = state.db.lock().unwrap();
                let new_url = format!("{working_mirror}{path}");
                if new_url != s.url {
                    db.update_series_url(s.id, &new_url).map_err(|e| e.to_string())?;
                }
                // De-duplicate by episode number, not URL: the site may have
                // moved to a new domain (the URLs all changed), so a URL-keyed
                // diff would re-insert every episode as a new unseen duplicate.
                let known = db.existing_episode_numbers(s.id).map_err(|e| e.to_string())?;
                // Episodes we already have get their link refreshed in place
                // (the domain/path may have changed) without touching seen.
                for e in eps.iter().filter(|e| known.contains(&e.number)) {
                    db.refresh_episode_meta(s.id, &e.number, &e.url, e.title.as_deref(), e.released_at.as_deref())
                        .map_err(|e| e.to_string())?;
                }
                for mut e in new_episodes(&eps, &known) {
                    e.series_id = s.id;
                    db.insert_episode(&e).map_err(|e| e.to_string())?;
                    total_new += 1;
                }
                // Only on a *successful* fetch — a failed one leaves the old
                // timestamp so the finished-show recheck retries next cycle.
                db.set_last_checked_at(s.id).map_err(|e| e.to_string())?;

                // Apply-once cross-site progress carry-over: if this series'
                // follow was carried from another site, mark every episode up
                // to the carried watermark seen now that its episode list has
                // been fetched, then clear the marker so it never re-fires.
                if let Some(n) = db.take_carried_seen_number(s.id).map_err(|e| e.to_string())? {
                    db.set_seen_cascade(s.id, &n.to_string(), true).map_err(|e| e.to_string())?;
                }
            }

            // One cover fetch per followed series per refresh — never in
            // bulk, never concurrent with another cover fetch (this inner
            // loop is sequential). Skip once it's already a fetched data:
            // URI; a failure here just leaves the remote (broken) url in
            // place to retry next time, it never blocks episode updates.
            if let Some(remote) = &s.cover_url {
                if !remote.starts_with("data:") {
                    if let Ok(data_uri) = fetch_cover_image(&app, remote).await {
                        let db = state.db.lock().unwrap();
                        let _ = db.update_series_cover(s.id, &data_uri);
                    }
                }
            }

            // One genre/kind backfill per followed series per refresh —
            // only for series that don't already have series_genres rows.
            // A failure here is silent and never blocks episode updates.
            backfill_series_genre_if_missing(&app, &state.db, &mirrors, s, a.as_ref()).await;
        }

        // Polite delay once per chunk rather than once per series — a chunk
        // already spaces its own concurrent requests out with real work
        // (page parse, DB writes, cover fetch), so this is deliberately not
        // simply "old delay ÷ concurrency"; halving it keeps a comparable
        // pace to before while trimming the fixed idle time in half.
        tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    }
    emit_refresh_progress(&app, total_series, total_series, "Completado");
    eprintln!(
        "[scrape] refresh() wall time: {:?} for {total_series} followed series ({skipped} skipped), {total_new} new episodes",
        refresh_started.elapsed()
    );

    // Opportunistic cloud backup (throttled 24h, only if changed & connected)
    // — fire-and-forget so a slow/failed upload never delays this command's
    // return to the UI.
    {
        let app2 = app.clone();
        tauri::async_runtime::spawn(async move {
            let _ = auto_backup_if_due(app2).await;
        });
    }

    Ok(total_new)
}

/// All episodes of a series (progress view), oldest first.
#[tauri::command]
pub fn list_episodes(state: State<'_, AppState>, series_id: i64) -> Result<Vec<Episode>, String> {
    let db = state.db.lock().unwrap();
    db.list_series_episodes(series_id).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_783_400_000;
    const DAY: i64 = 86_400;

    /// Baseline "quiet week" listing series: next episode in the future and
    /// the site's badge shows the *upcoming* episode's number (db_max+1, the
    /// live-verified up-to-date pattern) — the case that must skip for the
    /// big win to exist. The unknown-badge ("??") case used to live here too,
    /// but Bug A (2026-07-12 airing-refresh fix) showed a future countdown
    /// alone can't justify skipping when the badge gives no signal at all —
    /// see `unknown_badge_with_future_countdown_uses_bounded_recheck` below.
    #[test]
    fn skip_when_next_episode_in_future_and_no_count_conflict() {
        // .sb = db_max+1: up to date under the verified next-episode-number
        // semantics (8/8 provably-current live series showed exactly this).
        assert!(!should_fetch_series(
            false, true, true, Some(NOW + DAY), Some(6), 5, None, NOW
        ));
        // .sb = db_max: db numbering equal/ahead of the badge — also nothing new.
        assert!(!should_fetch_series(
            false, true, true, Some(NOW + DAY), Some(5), 5, None, NOW
        ));
    }

    /// Bug A (2026-07-12 airing-refresh-missing-episodes fix): a non-numeric
    /// "??" badge gives no count signal at all, and the site's countdown is
    /// *never* observed in the past (it always rolls forward the instant an
    /// episode posts, live-verified), so "future countdown -> skip" would
    /// skip such a series FOREVER — proven live against "One Piece: Arco de
    /// Elbaph" (203 episodes, airing, on-listing, badge NULL, future
    /// next_episode_at, permanently stuck until a force refresh). Fall back
    /// to a bounded recheck instead of an indefinite skip.
    #[test]
    fn unknown_badge_with_future_countdown_uses_bounded_recheck() {
        // Never checked before -> fetch (matches the old test's flipped case:
        // unknown badge + never checked used to assert skip, which was the
        // bug).
        assert!(should_fetch_series(
            false, true, true, Some(NOW + DAY), None, 5, None, NOW
        ));
        // Checked well within the 6h window -> skip.
        assert!(!should_fetch_series(
            false, true, true, Some(NOW + DAY), None, 5, Some(3600), NOW
        ));
        // Checked exactly 6h ago -> fetch (boundary is inclusive).
        assert!(should_fetch_series(
            false, true, true, Some(NOW + DAY), None, 5, Some(6 * 3600), NOW
        ));
        // Checked well past 6h ago -> fetch.
        assert!(should_fetch_series(
            false, true, true, Some(NOW + DAY), None, 5, Some(7 * 3600), NOW
        ));
    }

    /// Bug B regression (2026-07-12 fix): `db_max_episode_number` must be the
    /// highest real episode NUMBER, not a row COUNT — recap/"0"/version rows
    /// inflate a row count and silently cancel the `+1` detection margin.
    /// Live proof: Tsue to Tsurugi no Wistoria Temporada 2 has episode rows
    /// "0|Recap", 1..12 (13 rows, highest real number 12); when episode 13
    /// posts the badge becomes 14.
    #[test]
    fn regression_recap_row_no_longer_masks_a_new_episode() {
        // Caught up: badge 13 (next-episode-number) vs max number 12 -> skip.
        assert!(!should_fetch_series(
            false, true, true, Some(NOW + DAY), Some(13), 12, None, NOW
        ));
        // Episode 13 just posted: badge rolls to 14 vs max number still 12 ->
        // fetch. Under the old row-COUNT bug (13) this was `14 > 13+1=14` =
        // false, i.e. the miss this fix corrects.
        assert!(should_fetch_series(
            false, true, true, Some(NOW + DAY), Some(14), 12, None, NOW
        ));
    }

    #[test]
    fn fetch_when_next_episode_in_past_and_count_unknown() {
        assert!(should_fetch_series(
            false, true, true, Some(NOW - 3600), None, 5, None, NOW
        ));
    }

    /// A countdown sitting in the past means the episode aired and the card
    /// hasn't rolled over — always fetch, regardless of what the badge says
    /// (the badge may not have rolled yet either).
    #[test]
    fn fetch_when_countdown_in_past_even_if_badge_looks_current() {
        assert!(should_fetch_series(
            false, true, true, Some(NOW - 3600), Some(6), 5, None, NOW
        ));
        assert!(should_fetch_series(
            false, true, true, Some(NOW - 3600), Some(5), 5, None, NOW
        ));
    }

    /// .sb greater than db+1 means the site has posted episodes we don't
    /// have (badge = next episode's number, so badge > db+1 ⇒ posted > db).
    /// This must win even over a future countdown: the site rolls the
    /// countdown to next week when it posts, so "future next_episode_at"
    /// alone would skip a freshly-posted episode forever.
    #[test]
    fn fetch_when_badge_says_site_is_ahead_even_with_future_countdown() {
        assert!(should_fetch_series(
            false, true, true, Some(NOW + DAY), Some(7), 5, None, NOW
        ));
    }

    #[test]
    fn fetch_when_no_signal_at_all_on_listing() {
        // On the listing but no countdown span and a non-numeric count
        // ("??") — no basis to skip.
        assert!(should_fetch_series(false, true, true, None, None, 5, None, NOW));
    }

    /// Followed series absent from the airing listing: fetch at most once
    /// per OFF_LISTING_RECHECK interval (24h — live verification showed the
    /// listing only carries ~77 series and genuinely-airing followed shows
    /// can be absent from it, so "absent = finished" is false and a 7-day
    /// interval would delay real episodes up to a week).
    #[test]
    fn off_listing_series_fetches_only_when_recheck_interval_elapsed() {
        // Never checked → fetch.
        assert!(should_fetch_series(false, true, false, None, None, 5, None, NOW));
        // Checked 1 hour ago → skip.
        assert!(!should_fetch_series(
            false, true, false, None, None, 5, Some(3600), NOW
        ));
        // Checked 25 hours ago → fetch again.
        assert!(should_fetch_series(
            false, true, false, None, None, 5, Some(25 * 3600), NOW
        ));
    }

    #[test]
    fn force_always_fetches() {
        // force=true overrides every skip rule, including the strongest ones.
        assert!(should_fetch_series(
            true, true, true, Some(NOW + DAY), Some(5), 5, Some(0), NOW
        ));
        assert!(should_fetch_series(true, true, false, None, None, 5, Some(0), NOW));
    }

    #[test]
    fn listing_scan_failure_falls_back_to_fetching_everything() {
        // If the airing-listing scan failed there is no fresh metadata; the
        // refresh must behave exactly like the pre-skip-logic code and fetch
        // every followed series rather than trusting stale skip signals.
        assert!(should_fetch_series(
            false, false, true, Some(NOW + DAY), Some(5), 5, Some(0), NOW
        ));
    }
}
