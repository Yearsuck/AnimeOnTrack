use super::*;
use crate::download_sources::{
    candidates, mytsumi_direct_media, parse_animeyt_container_url, parse_animeflv_servers, parse_jkanime_servers, parse_mytsumi_tabs,
    parse_tioanime_servers, resolve_from_page, supported_host, voe_redirect_target, Media, Server,
};
use crate::hls::{parse_gogoanime_player_url, parse_megaplay_playlist_url, parse_wrapper_iframe};
use crate::downloader::{self, DlError};
use scraper::{Html, Selector};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock};

/// Cancel flags of the downloads that are queued or running, by episode id.
static ACTIVE: LazyLock<Mutex<HashMap<i64, Arc<AtomicBool>>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
/// One transfer at a time: several parallel multi-hundred-MB downloads from the
/// same mirror get throttled, and the second one would only slow the first.
static SLOT: LazyLock<tokio::sync::Semaphore> = LazyLock::new(|| tokio::sync::Semaphore::new(1));

#[derive(Serialize, Clone)]
struct DownloadProgress {
    episode_id: i64,
    /// `downloading` | `downloaded` | `failed` | `cancelled`
    status: &'static str,
    percent: Option<u8>,
    error: Option<String>,
}

fn emit(app: &AppHandle, episode_id: i64, status: &'static str, percent: Option<u8>, error: Option<String>) {
    let _ = app.emit("download-progress", DownloadProgress { episode_id, status, percent, error });
}

/// Download an episode's video for offline viewing.
///
/// Returns as soon as the download is queued; progress and the outcome arrive
/// as `download-progress` events and in the episode row's `download_status`.
/// TioAnime, JKAnime and animeyt.cc episode pages are understood; the hosts that
/// hand out a plain file are YourUpload, Mp4upload, Mediafire, Streamtape and
/// the direct mp4 tab of animeyt.cc's player. Anything else reports a reason.
#[tauri::command]
pub async fn download_episode(
    app: AppHandle,
    _state: State<'_, AppState>,
    episode_id: i64,
) -> Result<(), String> {
    enqueue(&app, episode_id)
}

/// Validate, mark `downloading` and hand the episode to the background task.
/// Shared by the single-episode command and the "download all pending" batch.
fn enqueue(app: &AppHandle, episode_id: i64) -> Result<(), String> {
    {
        let state = app.state::<AppState>();
        let db = state.db.lock().unwrap();
        let ep = db.get_episode(episode_id).map_err(|e| e.to_string())?;
        if ep.download_status.as_deref() == Some("downloading") {
            return Err("Already downloading this episode".into());
        }
        if ep.download_status.as_deref() == Some("downloaded") {
            return Err("Episode is already downloaded".into());
        }
        db.set_download_status(episode_id, "downloading", None).map_err(|e| e.to_string())?;
    }

    let cancel = Arc::new(AtomicBool::new(false));
    ACTIVE.lock().unwrap().insert(episode_id, cancel.clone());
    emit(app, episode_id, "downloading", None, None);

    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let outcome = run_download(&app, episode_id, &cancel).await;
        ACTIVE.lock().unwrap().remove(&episode_id);
        let state = app.state::<AppState>();
        match outcome {
            Ok(path) => {
                let _ = state.db.lock().unwrap().set_download_status(episode_id, "downloaded", Some(&path));
                crate::file_log::info(&format!("download finished: episode {episode_id}"));
                emit(&app, episode_id, "downloaded", Some(100), None);
            }
            Err(DlError::Cancelled) => {
                let _ = state.db.lock().unwrap().clear_download_columns(&[episode_id]);
                emit(&app, episode_id, "cancelled", None, None);
            }
            Err(e) => {
                let _ = state.db.lock().unwrap().set_download_status(episode_id, "failed", None);
                crate::file_log::warn(&format!("download failed: episode {episode_id}: {e}"));
                emit(&app, episode_id, "failed", None, Some(e.to_string()));
            }
        }
    });
    Ok(())
}

/// Whether the downloader understands this episode link.
fn is_supported_episode_url(url: &str) -> bool {
    site_of(url).is_some()
}

/// Plans the ordered sequence of URLs to attempt for an episode download.
///
/// The episode's own URL is attempted first if supported by the downloader (`is_supported_episode_url`),
/// so the user's active site preference is preserved.
/// Sibling URLs on other sites are then ordered by the user's configured `priority`
/// (sites not present in `priority` sort last in stable order).
/// URLs on unsupported sites (e.g. AnimeLand) are dropped, and duplicate URLs are deduplicated.
pub(crate) fn plan_download_urls(
    own_url: &str,
    siblings: &[(String, String)],
    priority: &[String],
) -> Vec<String> {
    let mut result = Vec::new();
    let mut seen = std::collections::HashSet::new();

    if is_supported_episode_url(own_url) && seen.insert(own_url.to_string()) {
        result.push(own_url.to_string());
    }

    let mut eligible: Vec<(usize, usize, &String)> = siblings
        .iter()
        .enumerate()
        .filter(|(_, (_site_id, url))| is_supported_episode_url(url))
        .map(|(idx, (site_id, url))| {
            let rank = priority
                .iter()
                .position(|p| p == site_id)
                .unwrap_or(usize::MAX);
            (rank, idx, url)
        })
        .collect();

    eligible.sort_by_key(|&(rank, idx, _)| (rank, idx));

    for (_, _, url) in eligible {
        if seen.insert(url.clone()) {
            result.push(url.clone());
        }
    }

    result
}

/// Picks the episode URL from a list of `(number, url)` pairs whose parsed episode number
/// matches the `canonical` target within a tolerance of 1e-6.
pub(crate) fn pick_episode_url(episodes: &[(String, String)], canonical: f64) -> Option<String> {
    for (num_str, url) in episodes {
        if let Some(parsed) = crate::db::parse_ep_number(num_str) {
            if (parsed - canonical).abs() < 1e-6 {
                return Some(url.clone());
            }
        }
    }
    None
}

#[derive(Serialize)]
pub struct DownloadBatch {
    /// Episodes put in the queue just now.
    pub queued: u32,
    /// Pending episodes whose site the downloader does not understand yet.
    pub unsupported: u32,
    /// Pending episodes that are already downloaded or already in the queue.
    pub already: u32,
}

/// Queue every episode of a series that is still unwatched and not yet
/// downloaded, oldest first (the queue runs one transfer at a time, in order).
#[tauri::command]
pub async fn download_pending(
    app: AppHandle,
    state: State<'_, AppState>,
    series_id: i64,
) -> Result<DownloadBatch, String> {
    let (episodes, priority) = {
        let db = state.db.lock().unwrap();
        let eps = db.list_series_episodes(series_id).map_err(|e| e.to_string())?;
        let known: Vec<&str> = crate::adapter::all_sites().iter().map(|s| s.id).collect();
        let priority = db.get_site_priority(&known).unwrap_or_default();
        (eps, priority)
    };
    let mut batch = DownloadBatch { queued: 0, unsupported: 0, already: 0 };
    let mut cached_sids: Option<Vec<i64>> = None;
    for ep in episodes.iter().filter(|e| !e.seen) {
        match ep.download_status.as_deref() {
            Some("downloading") | Some("downloaded") => batch.already += 1,
            _ => {
                let has_candidate = if is_supported_episode_url(&ep.url) {
                    true
                } else {
                    let sids = match &cached_sids {
                        Some(s) => s.as_slice(),
                        None => {
                            let s = {
                                let db = state.db.lock().unwrap();
                                db.canonical_series_ids(series_id)
                                    .map_err(|e| e.to_string())?
                            };
                            cached_sids = Some(s);
                            cached_sids.as_deref().unwrap()
                        }
                    };
                    let siblings = {
                        let db = state.db.lock().unwrap();
                        db.sibling_episodes_in(ep.id, sids)
                            .map_err(|e| e.to_string())?
                            .into_iter()
                            .map(|s| (s.site_id, s.url))
                            .collect::<Vec<_>>()
                    };
                    !plan_download_urls(&ep.url, &siblings, &priority).is_empty()
                };

                if !has_candidate {
                    batch.unsupported += 1;
                } else if enqueue(&app, ep.id).is_ok() {
                    batch.queued += 1;
                }
            }
        }
    }
    Ok(batch)
}

/// Delete every downloaded file of a series (and cancel its running or queued
/// downloads) without touching the episodes' watched state. Returns how many
/// episodes were cleaned up.
#[tauri::command]
pub async fn delete_series_downloads(
    state: State<'_, AppState>,
    series_id: i64,
) -> Result<u32, String> {
    let db = state.db.lock().unwrap();
    let episodes = db.list_series_episodes(series_id).map_err(|e| e.to_string())?;
    let mut cleaned = 0u32;
    for ep in &episodes {
        if ep.download_status.as_deref() == Some("downloading") {
            // The running task removes its `.part` file and clears the row;
            // a stale flag-less row (e.g. after a crash) is cleared here.
            match ACTIVE.lock().unwrap().get(&ep.id) {
                Some(flag) => flag.store(true, Ordering::SeqCst),
                None => {
                    db.clear_download_columns(&[ep.id]).map_err(|e| e.to_string())?;
                }
            }
            cleaned += 1;
        } else if ep.download_path.is_some() || ep.download_status.is_some() {
            if let Some(path) = &ep.download_path {
                let _ = std::fs::remove_file(path);
            }
            db.clear_download_columns(&[ep.id]).map_err(|e| e.to_string())?;
            cleaned += 1;
        }
    }
    Ok(cleaned)
}

async fn attempt(
    app: &AppHandle,
    url: &str,
    episode_id: i64,
    base: &std::path::Path,
    cancel: &Arc<AtomicBool>,
) -> Result<String, DlError> {
    let media = resolve_media(app, url).await?;
    if cancel.load(Ordering::SeqCst) {
        return Err(DlError::Cancelled);
    }

    let mut last_pct: i16 = -1;
    let app_for_progress = app.clone();
    let progress = move |done: u64, total: Option<u64>| {
        let Some(total) = total.filter(|t| *t > 0) else { return };
        let pct = ((done.saturating_mul(100)) / total).min(100) as i16;
        if pct != last_pct {
            last_pct = pct;
            emit(&app_for_progress, episode_id, "downloading", Some(pct as u8), None);
        }
    };
    let is_hls = media.host == "hls"
        || url::Url::parse(&media.url)
            .map(|u| u.path().to_ascii_lowercase().ends_with(".m3u8"))
            .unwrap_or(false);
    let final_file = if is_hls {
        downloader::fetch_hls_to_file(&media.url, &media.referer, base, cancel, progress).await?
    } else {
        downloader::fetch_to_file(&media, base, cancel, progress).await?
    };
    Ok(final_file.to_string_lossy().into_owned())
}

/// On-demand scraping of sibling series episode lists when DB candidate downloads failed.
///
/// Queries `siblings_to_scrape`, orders them by user priority, checks downloader support,
/// caps at 3 rows, and fetches/diffs episodes on demand. A user-initiated download bypasses
/// the domain breaker cooldown so explicit user intent is always attempted.
/// Stops and returns at the first candidate that downloads successfully.
#[allow(clippy::too_many_arguments)]
async fn scrape_and_try_siblings(
    app: &AppHandle,
    episode_id: i64,
    episode_url: &str,
    priority: &[String],
    base: &std::path::Path,
    cancel: &Arc<AtomicBool>,
    attempted: &mut std::collections::HashSet<String>,
    errors: &mut Vec<(String, DlError)>,
) -> Result<Option<String>, DlError> {
    if cancel.load(Ordering::SeqCst) {
        return Err(DlError::Cancelled);
    }

    let to_scrape = {
        let state = app.state::<AppState>();
        let db = state.db.lock().unwrap();
        db.siblings_to_scrape(episode_id)
            .map_err(|e| DlError::Io(e.to_string()))?
    };

    crate::file_log::info(&format!(
        "download fallback: siblings_to_scrape returned {} row(s) for episode {episode_id}",
        to_scrape.len()
    ));

    let mut eligible = Vec::new();
    for (idx, row) in to_scrape.into_iter().enumerate() {
        let site = if !row.site_id.is_empty() {
            row.site_id.clone()
        } else {
            site_label(&row.url)
        };

        if !is_supported_episode_url(&row.url) {
            crate::file_log::info(&format!(
                "download fallback: skipped series {} on {site} (unsupported site)",
                row.series_id
            ));
            continue;
        }

        if crate::adapter::adapter_for(&row.site_id).is_none() {
            crate::file_log::info(&format!(
                "download fallback: skipped series {} on {site} (no adapter)",
                row.series_id
            ));
            continue;
        }

        let rank = priority
            .iter()
            .position(|p| p == &row.site_id)
            .unwrap_or(usize::MAX);
        eligible.push((rank, idx, row));
    }

    eligible.sort_by_key(|&(rank, idx, _)| (rank, idx));

    let rows_to_scrape: Vec<_> = eligible.into_iter().map(|(_, _, row)| row).take(3).collect();

    let mut unaligned_sites = Vec::new();
    for row in rows_to_scrape {
        if cancel.load(Ordering::SeqCst) {
            return Err(DlError::Cancelled);
        }

        let site = if !row.site_id.is_empty() {
            row.site_id.clone()
        } else {
            site_label(&row.url)
        };

        let a = match crate::adapter::adapter_for(&row.site_id) {
            Some(adapter) => adapter,
            None => {
                crate::file_log::info(&format!(
                    "download fallback: skipped series {} on {site} (no adapter)",
                    row.series_id
                ));
                continue;
            }
        };

        let scraped = match fetch_html_with_script(app, &row.url, a.episode_fetch_script()).await {
            Ok(s) => {
                crate::domain_breaker::record_success(&row.url);
                s
            }
            Err(e) => {
                crate::domain_breaker::record_failure(&row.url);
                crate::file_log::info(&format!(
                    "download fallback: scrape failed for series {} on {site}: {e}",
                    row.series_id
                ));
                errors.push((
                    site,
                    DlError::Net(format!("could not load episode list: {e}")),
                ));
                continue;
            }
        };

        let episodes = match a.parse_series(scraped.extra.as_deref().unwrap_or(&scraped.html)) {
            Ok(eps) => eps,
            Err(e) => {
                crate::file_log::info(&format!(
                    "download fallback: parse failed for series {} on {site}: {e}",
                    row.series_id
                ));
                errors.push((
                    site,
                    DlError::Net(format!("could not load episode list: {e}")),
                ));
                continue;
            }
        };

        crate::file_log::info(&format!(
            "download fallback: scraped series {} on {site}, parsed {} episode(s)",
            row.series_id,
            episodes.len()
        ));

        // `fetch_html_with_script` cannot be interrupted, so honour a cancel that
        // arrived during the scrape before writing anything.
        if cancel.load(Ordering::SeqCst) {
            return Err(DlError::Cancelled);
        }

        {
            let state = app.state::<AppState>();
            let db = state.db.lock().unwrap();
            if let Err(e) = db.apply_episode_diff(row.series_id, None, &episodes) {
                crate::file_log::info(&format!(
                    "download fallback: apply diff failed for series {} on {site}: {e}",
                    row.series_id
                ));
                errors.push((
                    site,
                    DlError::Net(format!("could not load episode list: {e}")),
                ));
                continue;
            }
            // Same as the episode fallback after a refetch: carry the watched
            // progress of the sibling's twins onto the freshly inserted rows so
            // they do not surface as unseen in Pendientes.
            let _ = db.sync_seen_progress_across_sites();
        }

        let updated_siblings = {
            let state = app.state::<AppState>();
            let db = state.db.lock().unwrap();
            db.sibling_episodes(episode_id)
                .map_err(|e| DlError::Io(e.to_string()))?
                .into_iter()
                .map(|s| (s.site_id, s.url))
                .collect::<Vec<_>>()
        };

        let new_candidates: Vec<String> = plan_download_urls(episode_url, &updated_siblings, priority)
            .into_iter()
            .filter(|url| !attempted.contains(url))
            .collect();

        crate::file_log::info(&format!(
            "download fallback: new candidate urls for episode {episode_id} from {site}: {}",
            if new_candidates.is_empty() {
                "none".to_string()
            } else {
                new_candidates.join(", ")
            }
        ));

        if new_candidates.is_empty() {
            unaligned_sites.push(site);
        }

        for candidate_url in new_candidates {
            if cancel.load(Ordering::SeqCst) {
                return Err(DlError::Cancelled);
            }
            attempted.insert(candidate_url.clone());
            let cand_site = site_label(&candidate_url);
            match attempt(app, &candidate_url, episode_id, base, cancel).await {
                Ok(final_path) => {
                    crate::file_log::info(&format!(
                        "download finished: episode {episode_id} via {cand_site}"
                    ));
                    return Ok(Some(final_path));
                }
                Err(DlError::Cancelled) => {
                    return Err(DlError::Cancelled);
                }
                Err(err) => {
                    crate::file_log::info(&format!(
                        "download fallback: episode {episode_id} failed on {cand_site} ({err}), trying next"
                    ));
                    emit(app, episode_id, "downloading", None, None);
                    errors.push((cand_site, err));
                }
            }
        }
    }

    for site in unaligned_sites {
        if !errors.iter().any(|(s, _)| s == &site) {
            errors.push((site, DlError::Net("episode not listed".to_string())));
        }
    }

    Ok(None)
}

/// Third phase of download fallback: searches other supported sites with no existing
/// `series` row for the anime, locates the canonical episode, and attempts to download it.
/// Does not write anything to the database (transient search only).
#[allow(clippy::too_many_arguments)]
async fn search_other_sites_and_try(
    app: &AppHandle,
    episode_id: i64,
    priority: &[String],
    base: &std::path::Path,
    cancel: &Arc<AtomicBool>,
    attempted: &mut std::collections::HashSet<String>,
    errors: &mut Vec<(String, DlError)>,
) -> Result<Option<String>, DlError> {
    if cancel.load(Ordering::SeqCst) {
        return Err(DlError::Cancelled);
    }

    let context = {
        let state = app.state::<AppState>();
        let db = state.db.lock().unwrap();
        db.download_search_context(episode_id)
            .map_err(|e| DlError::Io(e.to_string()))?
    };

    let Some(context) = context else {
        return Ok(None);
    };

    let (primary_title, english_title) = &context.titles;

    let mut eligible = Vec::new();
    for (idx, site) in crate::adapter::all_sites().iter().enumerate() {
        if context.group_site_ids.iter().any(|gid| gid == site.id) {
            continue;
        }
        if !is_supported_episode_url(site.default_base_url) {
            continue;
        }
        let rank = priority
            .iter()
            .position(|p| p == site.id)
            .unwrap_or(usize::MAX);
        eligible.push((rank, idx, site));
    }
    eligible.sort_by_key(|&(rank, idx, _)| (rank, idx));

    let sites_to_try: Vec<&crate::adapter::SiteInfo> =
        eligible.into_iter().map(|(_, _, s)| s).take(3).collect();

    for site in sites_to_try {
        if cancel.load(Ordering::SeqCst) {
            return Err(DlError::Cancelled);
        }

        let a = match crate::adapter::adapter_for(site.id) {
            Some(adapter) => adapter,
            None => continue,
        };

        let mirrors = {
            let state = app.state::<AppState>();
            let db = state.db.lock().unwrap();
            crate::commands::mirrors::load_mirrors(&db, site.id)
                .ok()
                .filter(|v| !v.is_empty())
                .unwrap_or_else(|| vec![site.default_base_url.to_string()])
        };

        crate::file_log::info(&format!(
            "download fallback: searching {} for \"{}\"",
            site.id, primary_title
        ));

        let mut cards = match crate::commands::follow::search_site(app, &mirrors, primary_title, a.as_ref()).await {
            Ok(c) => c,
            Err(e) => {
                crate::file_log::info(&format!(
                    "download fallback: search on {} failed: {e}",
                    site.id
                ));
                errors.push((site.id.to_string(), DlError::Net(format!("search failed: {e}"))));
                continue;
            }
        };

        let mut best = crate::matching::best_match(
            &[primary_title],
            &crate::commands::discover::to_candidates(&cards),
        );

        if best.is_none() {
            if let Some(english) = english_title {
                if !english.eq_ignore_ascii_case(primary_title) {
                    if cancel.load(Ordering::SeqCst) {
                        return Err(DlError::Cancelled);
                    }
                    crate::file_log::info(&format!(
                        "download fallback: searching {} for \"{}\"",
                        site.id, english
                    ));
                    match crate::commands::follow::search_site(app, &mirrors, english, a.as_ref()).await {
                        Ok(eng_cards) => {
                            cards = eng_cards;
                            best = crate::matching::best_match(
                                &[primary_title],
                                &crate::commands::discover::to_candidates(&cards),
                            );
                        }
                        Err(e) => {
                            crate::file_log::info(&format!(
                                "download fallback: search on {} failed: {e}",
                                site.id
                            ));
                            errors.push((site.id.to_string(), DlError::Net(format!("search failed: {e}"))));
                            continue;
                        }
                    }
                }
            }
        }

        let Some(m) = best else {
            crate::file_log::info(&format!(
                "download fallback: search on {} yielded no match",
                site.id
            ));
            errors.push((site.id.to_string(), DlError::Net("show not found".into())));
            continue;
        };

        let matched = &cards[m.index];
        let catalog_refs: Vec<&str> = std::iter::once(primary_title.as_str())
            .chain(english_title.as_deref())
            .collect();
        if !crate::matching::season_markers_compatible(&matched.title, &catalog_refs) {
            crate::file_log::info(&format!(
                "download fallback: {} match \"{}\" is another season, skipped",
                site.id, matched.title
            ));
            errors.push((site.id.to_string(), DlError::Net("show not found".into())));
            continue;
        }
        crate::file_log::info(&format!(
            "download fallback: {} search matched {}",
            site.id, matched.url
        ));

        if cancel.load(Ordering::SeqCst) {
            return Err(DlError::Cancelled);
        }

        let eps = match crate::commands::scan::fetch_episode_list_for(app, &mirrors, &matched.url, a.as_ref()).await {
            Ok(eps) => {
                crate::domain_breaker::record_success(&matched.url);
                eps
            }
            Err(e) => {
                crate::domain_breaker::record_failure(&matched.url);
                crate::file_log::info(&format!(
                    "download fallback: episode list failed for {} on {}: {e}",
                    matched.url, site.id
                ));
                errors.push((site.id.to_string(), DlError::Net(format!("search failed: {e}"))));
                continue;
            }
        };

        let ep_tuples: Vec<(String, String)> = eps.into_iter().map(|e| (e.number, e.url)).collect();
        let candidate_url = match pick_episode_url(&ep_tuples, context.canonical_number) {
            Some(url) => url,
            None => {
                crate::file_log::info(&format!(
                    "download fallback: episode {} not listed on {}",
                    context.canonical_number, site.id
                ));
                errors.push((site.id.to_string(), DlError::Net("episode not listed".into())));
                continue;
            }
        };

        if attempted.contains(&candidate_url) {
            continue;
        }
        attempted.insert(candidate_url.clone());

        if cancel.load(Ordering::SeqCst) {
            return Err(DlError::Cancelled);
        }

        let cand_site = site_label(&candidate_url);
        match attempt(app, &candidate_url, episode_id, base, cancel).await {
            Ok(final_path) => {
                crate::file_log::info(&format!(
                    "download finished: episode {episode_id} via {cand_site}"
                ));
                return Ok(Some(final_path));
            }
            Err(DlError::Cancelled) => {
                return Err(DlError::Cancelled);
            }
            Err(err) => {
                crate::file_log::info(&format!(
                    "download fallback: episode {episode_id} failed on {cand_site} ({err}), trying next"
                ));
                emit(app, episode_id, "downloading", None, None);
                errors.push((cand_site, err));
            }
        }
    }

    Ok(None)
}

async fn run_download(app: &AppHandle, episode_id: i64, cancel: &Arc<AtomicBool>) -> Result<String, DlError> {
    let _slot = SLOT.acquire().await.map_err(|e| DlError::Net(e.to_string()))?;
    if cancel.load(Ordering::SeqCst) {
        return Err(DlError::Cancelled);
    }

    let (episode_url, series_id, siblings, priority) = {
        let state = app.state::<AppState>();
        let db = state.db.lock().unwrap();
        let ep = db.get_episode(episode_id).map_err(|e| DlError::Io(e.to_string()))?;
        let siblings = db
            .sibling_episodes(episode_id)
            .map_err(|e| DlError::Io(e.to_string()))?
            .into_iter()
            .map(|s| (s.site_id, s.url))
            .collect::<Vec<_>>();
        let known: Vec<&str> = crate::adapter::all_sites().iter().map(|s| s.id).collect();
        let priority = db.get_site_priority(&known).unwrap_or_default();
        (ep.url, ep.series_id, siblings, priority)
    };
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| DlError::Io(e.to_string()))?
        .join("downloads");
    // Created and locked down (no execute rights on its files) before anything lands in it.
    let hardened_dir = dir.clone();
    let _ = tokio::task::spawn_blocking(move || crate::download_safety::harden_downloads_dir(&hardened_dir)).await;
    let base = downloader::episode_file_base(&dir, series_id, episode_id);

    let candidates = plan_download_urls(&episode_url, &siblings, &priority);
    if candidates.is_empty() {
        return Err(DlError::Net(unsupported_site_error(&episode_url)));
    }

    let mut attempted: std::collections::HashSet<String> = candidates.iter().cloned().collect();
    let mut errors: Vec<(String, DlError)> = Vec::new();
    for (idx, candidate_url) in candidates.iter().enumerate() {
        if cancel.load(Ordering::SeqCst) {
            return Err(DlError::Cancelled);
        }
        let site = site_label(candidate_url);
        match attempt(app, candidate_url, episode_id, &base, cancel).await {
            Ok(final_path) => {
                if candidate_url != &episode_url {
                    crate::file_log::info(&format!(
                        "download finished: episode {episode_id} via {site}"
                    ));
                }
                return Ok(final_path);
            }
            Err(DlError::Cancelled) => {
                return Err(DlError::Cancelled);
            }
            Err(err) => {
                if idx + 1 < candidates.len() {
                    crate::file_log::info(&format!(
                        "download fallback: episode {episode_id} failed on {site} ({err}), trying next"
                    ));
                    emit(app, episode_id, "downloading", None, None);
                }
                errors.push((site, err));
            }
        }
    }

    if let Some(final_path) = scrape_and_try_siblings(
        app,
        episode_id,
        &episode_url,
        &priority,
        &base,
        cancel,
        &mut attempted,
        &mut errors,
    )
    .await?
    {
        return Ok(final_path);
    }

    if let Some(final_path) = search_other_sites_and_try(
        app,
        episode_id,
        &priority,
        &base,
        cancel,
        &mut attempted,
        &mut errors,
    )
    .await?
    {
        return Ok(final_path);
    }

    if errors.len() == 1 {
        let (_, single_err) = errors.pop().unwrap();
        Err(single_err)
    } else {
        let details = errors
            .iter()
            .map(|(site, err)| format!("{site} ({err})"))
            .collect::<Vec<_>>()
            .join("; ");
        Err(DlError::Net(format!("all sources failed: {details}")))
    }
}

/// The sites whose episode pages the downloader understands.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Site {
    TioAnime,
    JkAnime,
    AnimeYt,
    GogoAnime,
    AnimeFlv,
}

fn site_of(url: &str) -> Option<Site> {
    let host = url::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(|h| h.to_ascii_lowercase()))?;
    let is = |d: &str| host == d || host.ends_with(&format!(".{d}"));
    if is("tioanime.com") {
        Some(Site::TioAnime)
    } else if is("jkanime.net") {
        Some(Site::JkAnime)
    } else if is("animeyt.cc") {
        Some(Site::AnimeYt)
    } else if is("gogoanime.by") {
        Some(Site::GogoAnime)
    } else if is("animeflv.or.at") {
        Some(Site::AnimeFlv)
    } else {
        None
    }
}

fn site_label(url: &str) -> String {
    match site_of(url) {
        Some(Site::TioAnime) => "tioanime".to_string(),
        Some(Site::JkAnime) => "jkanime".to_string(),
        Some(Site::AnimeYt) => "animeytx".to_string(),
        Some(Site::GogoAnime) => "gogoanime".to_string(),
        Some(Site::AnimeFlv) => "animeflv".to_string(),
        None => url::Url::parse(url)
            .ok()
            .and_then(|u| u.host_str().map(|h| h.to_ascii_lowercase()))
            .unwrap_or_else(|| "unknown".to_string()),
    }
}

fn unsupported_site_error(url: &str) -> String {
    let host = url::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_owned))
        .unwrap_or_default();
    format!("downloads from {host} are not supported yet")
}

/// Formats a list of per-server outcomes into a single diagnostic string.
///
/// Each pair `(server, reason)` is formatted as `<server>: <reason>`, where
/// `reason` is capped to at most 120 characters to keep user-facing error messages
/// readable without panicking on multi-byte UTF-8 boundaries. Pairs are joined
/// by `; ` preserving their input order. Returns an empty string when the slice is empty.
pub(crate) fn format_server_errors<S1: AsRef<str>, S2: AsRef<str>>(errors: &[(S1, S2)]) -> String {
    errors
        .iter()
        .map(|(server, reason)| {
            let s = server.as_ref();
            let r = reason.as_ref();
            let capped: String = r.chars().take(120).collect();
            format!("{s}: {capped}")
        })
        .collect::<Vec<_>>()
        .join("; ")
}

/// Plain HTTP GET of an embed / host page (these hosts are not behind
/// Cloudflare, unlike the episode pages): browser UA, the previous page as
/// Referer, 20 s, size-capped. Returns `(final_url, body)`.
async fn fetch_text(url: &str, referer: &str) -> Result<(String, String), String> {
    const MAX_PAGE_BYTES: u64 = 3 * 1024 * 1024;
    if !is_safe_external_url(url) {
        return Err("not a public web address".into());
    }
    let resp = downloader::client()
        .get(url)
        .header(reqwest::header::USER_AGENT, downloader::browser_user_agent())
        .header(reqwest::header::REFERER, referer)
        .timeout(std::time::Duration::from_secs(20))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }
    if resp.content_length().is_some_and(|n| n > MAX_PAGE_BYTES) {
        return Err("page too large".into());
    }
    // The URL after redirects: relative links and Referer checks depend on it.
    let final_url = resp.url().to_string();
    let body = resp.text().await.map_err(|e| e.to_string())?;
    Ok((final_url, body))
}

/// Plain HTTP GET of a JSON endpoint: browser UA, the previous page as Referer,
/// Accept: application/json, 20 s timeout, size-capped. Returns body text.
async fn fetch_json_text(url: &str, referer: &str) -> Result<String, String> {
    const MAX_JSON_BYTES: u64 = 1024 * 1024;
    if !is_safe_external_url(url) {
        return Err("not a public web address".into());
    }
    let resp = downloader::client()
        .get(url)
        .header(reqwest::header::USER_AGENT, downloader::browser_user_agent())
        .header(reqwest::header::REFERER, referer)
        .header(reqwest::header::ACCEPT, "application/json")
        .timeout(std::time::Duration::from_secs(20))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }
    if resp.content_length().is_some_and(|n| n > MAX_JSON_BYTES) {
        return Err("response too large".into());
    }
    let body = resp.text().await.map_err(|e| e.to_string())?;
    if body.len() as u64 > MAX_JSON_BYTES {
        return Err("response too large".into());
    }
    Ok(body)
}

/// Parses the video source endpoint URL from a megavid.buzz player embed.
///
/// Returns `None` if the embed URL does not belong to `megavid.buzz` (or a subdomain),
/// or if `#player-payload` JSON with a non-empty `sourceUrl` is absent.
pub(crate) fn parse_megavid_source_url(embed_url: &str, embed_html: &str) -> Option<String> {
    let base = url::Url::parse(embed_url).ok()?;
    let host = base.host_str()?.to_ascii_lowercase();
    if !(host == "megavid.buzz" || host.ends_with(".megavid.buzz")) {
        return None;
    }

    let doc = Html::parse_document(embed_html);
    let sel = Selector::parse("#player-payload").ok()?;
    let el = doc.select(&sel).next()?;
    let text = el.text().collect::<String>();
    let val: serde_json::Value = serde_json::from_str(&text).ok()?;
    let source_url = val.get("sourceUrl").and_then(|v| v.as_str())?.trim();
    if source_url.is_empty() {
        return None;
    }

    let joined = base.join(source_url).ok()?;
    if joined.scheme() != "http" && joined.scheme() != "https" {
        return None;
    }
    Some(joined.to_string())
}

/// Parses the JSON response from megavid's source endpoint.
///
/// Returns the master playlist URL on `status == "ok"` with a non-empty `source`,
/// or an error description otherwise (including server `message` if present).
pub(crate) fn parse_megavid_source_response(json: &str) -> Result<String, String> {
    let val: serde_json::Value = serde_json::from_str(json)
        .map_err(|e| format!("invalid JSON response from video source: {e}"))?;

    let status = val.get("status").and_then(|v| v.as_str()).unwrap_or("");
    if status == "ok" {
        let source = val
            .get("source")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .unwrap_or("");
        if source.is_empty() {
            return Err("video source returned status 'ok' with empty source URL".into());
        }
        Ok(source.to_string())
    } else {
        let msg = val.get("message").and_then(|v| v.as_str()).unwrap_or("").trim();
        if !msg.is_empty() {
            Err(format!("video source error ({status}): {msg}"))
        } else if !status.is_empty() {
            Err(format!("video source returned status '{status}'"))
        } else {
            Err("video source returned unrecognized response".into())
        }
    }
}

/// GogoAnime: episode page -> its player wrapper -> the megaplay or megavid embed -> an
/// HLS playlist. Every hop is plain HTTP; the result is a `.m3u8` for the HLS downloader.
async fn resolve_gogoanime(episode_html: &str, episode_url: &str) -> Result<Media, DlError> {
    let player_url = parse_gogoanime_player_url(episode_html)
        .ok_or_else(|| DlError::Net("this episode has no player".into()))?;
    let (_, wrapper) = fetch_text(&player_url, episode_url)
        .await
        .map_err(|e| DlError::Net(format!("could not open the player: {e}")))?;
    let embed_url = parse_wrapper_iframe(&wrapper)
        .ok_or_else(|| DlError::Net("the player has no video frame".into()))?;
    let (_, embed) = fetch_text(&embed_url, &player_url)
        .await
        .map_err(|e| DlError::Net(format!("could not open the video frame: {e}")))?;

    if let Some(playlist) = parse_megaplay_playlist_url(&embed) {
        if !is_safe_external_url(&playlist) {
            return Err(DlError::Net("the stream is not a public web address".into()));
        }
        return Ok(Media { url: playlist, referer: "https://megaplay.su/".to_string(), host: "hls" });
    }

    if let Some(endpoint) = parse_megavid_source_url(&embed_url, &embed) {
        let json = fetch_json_text(&endpoint, &embed_url)
            .await
            .map_err(|e| DlError::Net(format!("could not retrieve the video source: {e}")))?;
        let source = parse_megavid_source_response(&json).map_err(DlError::Net)?;
        if !is_safe_external_url(&source) {
            return Err(DlError::Net("the stream is not a public web address".into()));
        }
        return Ok(Media { url: source, referer: "https://megavid.buzz/".to_string(), host: "hls" });
    }

    Err(DlError::Net("the video frame has no stream".into()))
}

/// Episode page -> list of mirrors -> first mirror that resolves to a plain file.
async fn resolve_media(app: &AppHandle, episode_url: &str) -> Result<Media, DlError> {
    if !is_safe_external_url(episode_url) {
        return Err(DlError::Net("the episode link is not a public web address".into()));
    }
    let Some(site) = site_of(episode_url) else {
        return Err(DlError::Net(unsupported_site_error(episode_url)));
    };

    // The episode page sits behind Cloudflare, so it goes through the browser
    // engine like every other page fetch.
    let page = fetch_html_with_script(app, episode_url, None)
        .await
        .map_err(|e| DlError::Net(format!("could not load the episode page: {e}")))?;

    if site == Site::GogoAnime {
        return resolve_gogoanime(&page.html, episode_url).await;
    }

    let servers = match site {
        Site::GogoAnime => unreachable!("handled above"),
        Site::TioAnime => parse_tioanime_servers(&page.html),
        Site::JkAnime => parse_jkanime_servers(&page.html),
        Site::AnimeFlv => parse_animeflv_servers(&page.html),
        Site::AnimeYt => {
            // animeyt.cc embeds one "container" player; its runtime page is plain
            // HTML listing the servers and, often, a direct mp4 tab.
            let container = parse_animeyt_container_url(&page.html)
                .ok_or_else(|| DlError::Net("this episode has no player container".into()))?;
            let (_, runtime) = fetch_text(&container, episode_url)
                .await
                .map_err(|e| DlError::Net(format!("could not open the player container: {e}")))?;
            let tabs = parse_mytsumi_tabs(&runtime);
            if let Some(media) = mytsumi_direct_media(&tabs) {
                if is_safe_external_url(&media.url) {
                    return Ok(media);
                }
            }
            tabs.into_iter()
                .filter(|t| !t.direct_video)
                .map(|t| Server { name: t.label, url: t.url })
                .collect()
        }
    };

    let url_host = url::Url::parse(episode_url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_owned))
        .unwrap_or_else(|| "unknown".to_string());

    let mut options = candidates(&servers);
    // Development aid: `AOT_DOWNLOAD_PREFER=<host id>` (e.g. `voe`, `mixdrop`) tries
    // that host's mirrors first, so one extractor can be exercised on a live page.
    if let Some(prefer) = std::env::var("AOT_DOWNLOAD_PREFER").ok().filter(|p| !p.trim().is_empty()) {
        options.sort_by_key(|s| supported_host(s) != Some(prefer.trim().to_ascii_lowercase().as_str()));
    }

    let mut outcomes: Vec<(String, String)> = Vec::new();
    'servers: for server in options {
        let Some(host) = supported_host(&server) else { continue };
        let (mut page_url, mut html) = match fetch_text(&server.url, episode_url).await {
            Ok(p) => p,
            Err(e) => {
                crate::file_log::info(&format!("download: {url_host} server {}: {e}", server.name));
                outcomes.push((server.name.clone(), e));
                continue;
            }
        };
        // VOE answers with a tiny page whose script redirects to the domain
        // that currently serves the video; follow it (at most twice).
        if host == "voe" {
            for _ in 0..2 {
                let Some(next) = voe_redirect_target(&html) else { break };
                match fetch_text(&next, &page_url).await {
                    Ok(p) => (page_url, html) = p,
                    Err(e) => {
                        crate::file_log::info(&format!("download: {url_host} server {}: {e}", server.name));
                        outcomes.push((server.name.clone(), e));
                        continue 'servers;
                    }
                }
            }
        }
        match resolve_from_page(host, &html, &page_url) {
            Some(media) if is_safe_external_url(&media.url) => return Ok(media),
            _ => {
                let reason = "no downloadable file".to_string();
                crate::file_log::info(&format!("download: {url_host} server {}: {reason}", server.name));
                outcomes.push((server.name.clone(), reason));
            }
        }
    }

    let mut seen_unsupported = std::collections::HashSet::new();
    for server in &servers {
        if supported_host(server).is_none() && seen_unsupported.insert(server.name.as_str()) {
            let reason = "unsupported host".to_string();
            crate::file_log::info(&format!("download: {url_host} server {}: {reason}", server.name));
            outcomes.push((server.name.clone(), reason));
        }
    }

    let combined_err = format_server_errors(&outcomes);
    if combined_err.is_empty() {
        let names: Vec<&str> = servers.iter().map(|s| s.name.as_str()).collect();
        Err(DlError::Net(format!(
            "no supported mirror on this episode (found: {})",
            if names.is_empty() { "none".to_string() } else { names.join(", ") }
        )))
    } else {
        Err(DlError::Net(combined_err))
    }
}

/// Open a downloaded episode with the system's default video player.
/// Only files inside the app's own downloads folder are ever opened.
#[tauri::command]
pub async fn play_download(
    app: AppHandle,
    state: State<'_, AppState>,
    episode_id: i64,
) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;
    let path = {
        let db = state.db.lock().unwrap();
        db.downloaded_paths(&[episode_id])
            .map_err(|e| e.to_string())?
            .into_iter()
            .next()
            .map(|(_, p)| p)
            .ok_or_else(|| "this episode is not downloaded".to_string())?
    };
    let downloads = app.path().app_data_dir().map_err(|e| e.to_string())?.join("downloads");
    let file = std::path::PathBuf::from(&path);
    if !file.starts_with(&downloads) || !file.is_file() {
        return Err("the downloaded file is missing".into());
    }
    app.opener().open_path(&path, None::<&str>).map_err(|e| e.to_string())
}

/// Cancel a queued or running download.
#[tauri::command]
pub async fn cancel_download(
    state: State<'_, AppState>,
    episode_id: i64,
) -> Result<(), String> {
    let flag = ACTIVE.lock().unwrap().get(&episode_id).cloned();
    match flag {
        // The running task notices the flag, removes its `.part` file and
        // clears the row itself.
        Some(flag) => {
            flag.store(true, Ordering::SeqCst);
            Ok(())
        }
        None => {
            let db = state.db.lock().unwrap();
            db.clear_download_columns(&[episode_id]).map_err(|e| e.to_string())
        }
    }
}

/// Delete a downloaded episode's file and clear its DB state.
#[tauri::command]
pub async fn delete_download(
    state: State<'_, AppState>,
    episode_id: i64,
) -> Result<(), String> {
    // A download still running for this episode must stop first, otherwise it
    // would finish later and bring the file back; its task cleans up after itself.
    if let Some(flag) = ACTIVE.lock().unwrap().get(&episode_id) {
        flag.store(true, Ordering::SeqCst);
        return Ok(());
    }
    let db = state.db.lock().unwrap();
    if let Ok(paths) = db.downloaded_paths(&[episode_id]) {
        for (_, path) in &paths {
            let _ = std::fs::remove_file(path);
        }
    }
    db.clear_download_columns(&[episode_id])
        .map_err(|e| e.to_string())
}

/// Total disk space used by all downloaded episode files.
#[tauri::command]
pub async fn get_downloads_size(
    state: State<'_, AppState>,
) -> Result<u64, String> {
    let db = state.db.lock().unwrap();
    db.downloads_disk_size().map_err(|e| e.to_string())
}

/// Delete ALL downloaded files and clear their DB columns.
#[tauri::command]
pub async fn clear_all_downloads(
    state: State<'_, AppState>,
) -> Result<usize, String> {
    let db = state.db.lock().unwrap();
    db.clear_all_downloads().map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn site_detection_covers_the_three_supported_sites_and_their_subdomains() {
        assert_eq!(site_of("https://tioanime.com/ver/x-1"), Some(Site::TioAnime));
        assert_eq!(site_of("https://www.tioanime.com/ver/x-1"), Some(Site::TioAnime));
        assert_eq!(site_of("https://jkanime.net/kanojo/2/"), Some(Site::JkAnime));
        assert_eq!(site_of("https://animeyt.cc/1/anime/x-capitulo-8/"), Some(Site::AnimeYt));
        assert_eq!(site_of("https://ANIMEYT.CC/x"), Some(Site::AnimeYt));
        assert_eq!(site_of("https://gogoanime.by/dark-machine-episode-1-english-subbed/"), Some(Site::GogoAnime));
    }

    #[test]
    fn site_detection_rejects_lookalikes_and_other_sites() {
        assert_eq!(site_of("https://tioanime.com.evil.example/x"), None);
        assert_eq!(site_of("https://notjkanime.net/x"), None);
        assert_eq!(site_of("https://animeflv.or.at/2026/10/11/x-episodio-3/"), Some(Site::AnimeFlv));
        assert_eq!(site_of("https://animeflv.or.at.evil.example/x"), None);
        assert_eq!(site_of("https://www4.animeflv.net/ver/x-1"), None);
        assert_eq!(site_of("https://gogoanime.by.evil.example/x"), None);
        assert_eq!(site_of("not a url"), None);
        assert!(!is_supported_episode_url("https://example.com/x"));
        assert!(is_supported_episode_url("https://jkanime.net/x/1/"));
    }

    #[test]
    fn plan_download_urls_own_first_and_siblings_ordered_by_priority_and_animeland_dropped() {
        let own = "https://tioanime.com/ver/x-1";
        let siblings = vec![
            ("animeytx".to_string(), "https://animeyt.cc/tv/x/1".to_string()),
            ("animeflv".to_string(), "https://animeflv.or.at/ver/x-1".to_string()),
            ("animeland".to_string(), "https://w7.animeland.tv/x-1".to_string()),
        ];
        let priority = vec!["animeflv".to_string(), "animeytx".to_string(), "tioanime".to_string()];
        let plan = plan_download_urls(own, &siblings, &priority);
        assert_eq!(
            plan,
            vec![
                "https://tioanime.com/ver/x-1",
                "https://animeflv.or.at/ver/x-1",
                "https://animeyt.cc/tv/x/1",
            ]
        );
    }

    #[test]
    fn plan_download_urls_unsupported_own_url_falls_back_to_siblings_in_priority_order() {
        let own = "https://w7.animeland.tv/x-1";
        let siblings = vec![
            ("tioanime".to_string(), "https://tioanime.com/ver/x-1".to_string()),
            ("animeflv".to_string(), "https://animeflv.or.at/ver/x-1".to_string()),
        ];
        let priority = vec!["animeflv".to_string(), "tioanime".to_string()];
        let plan = plan_download_urls(own, &siblings, &priority);
        assert_eq!(
            plan,
            vec![
                "https://animeflv.or.at/ver/x-1",
                "https://tioanime.com/ver/x-1",
            ]
        );
    }

    #[test]
    fn plan_download_urls_deduplicates_identical_urls() {
        let own = "https://tioanime.com/ver/x-1";
        let siblings = vec![
            ("tioanime".to_string(), "https://tioanime.com/ver/x-1".to_string()),
            ("animeflv".to_string(), "https://animeflv.or.at/ver/x-1".to_string()),
            ("animeytx".to_string(), "https://animeflv.or.at/ver/x-1".to_string()),
        ];
        let priority = vec!["animeflv".to_string(), "animeytx".to_string()];
        let plan = plan_download_urls(own, &siblings, &priority);
        assert_eq!(
            plan,
            vec![
                "https://tioanime.com/ver/x-1",
                "https://animeflv.or.at/ver/x-1",
            ]
        );
    }

    #[test]
    fn plan_download_urls_unknown_site_id_sorts_after_listed_priority() {
        let own = "https://tioanime.com/ver/x-1";
        let siblings = vec![
            ("unknown_site".to_string(), "https://jkanime.net/x/1/".to_string()),
            ("animeflv".to_string(), "https://animeflv.or.at/ver/x-1".to_string()),
        ];
        let priority = vec!["animeflv".to_string()];
        let plan = plan_download_urls(own, &siblings, &priority);
        assert_eq!(
            plan,
            vec![
                "https://tioanime.com/ver/x-1",
                "https://animeflv.or.at/ver/x-1",
                "https://jkanime.net/x/1/",
            ]
        );
    }

    #[test]
    fn plan_download_urls_returns_empty_when_nothing_supported() {
        let own = "https://w7.animeland.tv/x-1";
        let siblings = vec![
            ("animeland".to_string(), "https://w7.animeland.tv/alt/x-1".to_string()),
            ("unsupported".to_string(), "https://example.com/video/1".to_string()),
        ];
        let priority = vec!["animeflv".to_string(), "tioanime".to_string()];
        let plan = plan_download_urls(own, &siblings, &priority);
        assert!(plan.is_empty());
    }

    #[test]
    fn unsupported_site_error_formats_host() {
        assert_eq!(
            unsupported_site_error("https://w7.animeland.tv/x-1"),
            "downloads from w7.animeland.tv are not supported yet"
        );
        assert_eq!(
            unsupported_site_error("invalid-url"),
            "downloads from  are not supported yet"
        );
    }

    #[test]
    fn is_supported_episode_url_works_on_series_urls_by_host() {
        assert!(is_supported_episode_url("https://tioanime.com/anime/steel-ball-run"));
        assert!(is_supported_episode_url("https://animeflv.or.at/anime/steel-ball-run"));
        assert!(is_supported_episode_url("https://gogoanime.by/category/steel-ball-run"));
        assert!(is_supported_episode_url("https://animeyt.cc/tv/steel-ball-run"));
        assert!(is_supported_episode_url("https://jkanime.net/steel-ball-run"));
        assert!(!is_supported_episode_url("https://w7.animeland.tv/steel-ball-run"));
    }

    #[test]
    fn format_server_errors_empty_list_returns_empty_string() {
        let errors: &[(&str, &str)] = &[];
        assert_eq!(format_server_errors(errors), "");
    }

    #[test]
    fn format_server_errors_preserves_ordering_and_joins_with_semicolon() {
        let errors = vec![
            ("YourUpload", "HTTP 404 Not Found"),
            ("Voe", "HTTP 404 Not Found"),
            ("Mega", "unsupported host"),
            ("Netu", "unsupported host"),
        ];
        assert_eq!(
            format_server_errors(&errors),
            "YourUpload: HTTP 404 Not Found; Voe: HTTP 404 Not Found; Mega: unsupported host; Netu: unsupported host"
        );
    }

    #[test]
    fn format_server_errors_truncates_long_reason_at_120_chars() {
        let long_reason = "x".repeat(150);
        let errors = vec![("Voe", long_reason.as_str())];
        let expected = format!("Voe: {}", "x".repeat(120));
        assert_eq!(format_server_errors(&errors), expected);
    }

    #[test]
    fn format_server_errors_truncates_multibyte_utf8_safely() {
        let utf8_reason = "é".repeat(150);
        let errors = vec![("Server1", utf8_reason.as_str())];
        let expected = format!("Server1: {}", "é".repeat(120));
        assert_eq!(format_server_errors(&errors), expected);
    }

    #[test]
    fn format_server_errors_does_not_truncate_short_reason() {
        let errors = vec![("Server1", "Short reason")];
        assert_eq!(format_server_errors(&errors), "Server1: Short reason");
    }

    #[test]
    fn parse_megavid_source_url_resolves_payload_present() {
        let embed_url = "https://megavid.buzz/mal/61469/1/sub";
        let html = r#"<!DOCTYPE html>
<html>
<head>
    <script type="application/json" id="player-payload">{"sourceUrl":"/mal/61469/1/sub/source","subtitles":[]}</script>
</head>
<body></body>
</html>"#;
        assert_eq!(
            parse_megavid_source_url(embed_url, html),
            Some("https://megavid.buzz/mal/61469/1/sub/source".to_string())
        );
    }

    #[test]
    fn parse_megavid_source_url_returns_none_when_payload_absent() {
        let embed_url = "https://megavid.buzz/mal/61469/1/sub";
        let html = "<html><body><div>no player payload</div></body></html>";
        assert_eq!(parse_megavid_source_url(embed_url, html), None);
    }

    #[test]
    fn parse_megavid_source_url_returns_none_for_non_megavid_host() {
        let embed_url = "https://evil.example.com/mal/61469/1/sub";
        let html = r#"<script type="application/json" id="player-payload">{"sourceUrl":"/mal/61469/1/sub/source"}</script>"#;
        assert_eq!(parse_megavid_source_url(embed_url, html), None);
    }

    #[test]
    fn parse_megavid_source_response_extracts_source_on_ok() {
        let json = r#"{"status":"ok","source":"https://cdnx.aniwatchtv.site/uwu/master.m3u8","tracks":[]}"#;
        assert_eq!(
            parse_megavid_source_response(json).unwrap(),
            "https://cdnx.aniwatchtv.site/uwu/master.m3u8"
        );
    }

    #[test]
    fn parse_megavid_source_response_returns_err_on_missing_with_message() {
        let json = r#"{"status":"missing","message":"x"}"#;
        let err = parse_megavid_source_response(json).unwrap_err();
        assert!(err.contains('x'), "expected error to contain 'x', got: {err}");
    }

    #[test]
    fn parse_megavid_source_response_returns_err_on_countdown() {
        let json = r#"{"status":"countdown","iframeUrl":"https://megavid.buzz/countdown"}"#;
        let err = parse_megavid_source_response(json).unwrap_err();
        assert!(err.contains("countdown"), "expected error to contain countdown, got: {err}");
    }

    #[test]
    fn parse_megavid_source_response_returns_err_on_garbage() {
        let err = parse_megavid_source_response("not valid json at all").unwrap_err();
        assert!(!err.is_empty());
    }

    #[test]
    fn parse_megavid_source_response_returns_err_on_ok_with_empty_source() {
        let json = r#"{"status":"ok","source":""}"#;
        let err = parse_megavid_source_response(json).unwrap_err();
        assert!(err.contains("empty source"), "expected error to mention empty source, got: {err}");

        let json_spaces = r#"{"status":"ok","source":"   "}"#;
        assert!(parse_megavid_source_response(json_spaces).is_err());

        let json_missing_source = r#"{"status":"ok"}"#;
        assert!(parse_megavid_source_response(json_missing_source).is_err());
    }

    #[test]
    fn pick_episode_url_exact_match() {
        let episodes = vec![
            ("1".to_string(), "https://site/ep1".to_string()),
            ("2".to_string(), "https://site/ep2".to_string()),
        ];
        assert_eq!(
            pick_episode_url(&episodes, 1.0),
            Some("https://site/ep1".to_string())
        );
        assert_eq!(
            pick_episode_url(&episodes, 2.0),
            Some("https://site/ep2".to_string())
        );
    }

    #[test]
    fn pick_episode_url_leading_zero_style() {
        let episodes = vec![
            ("01".to_string(), "https://site/ep01".to_string()),
            ("02".to_string(), "https://site/ep02".to_string()),
        ];
        assert_eq!(
            pick_episode_url(&episodes, 1.0),
            Some("https://site/ep01".to_string())
        );
    }

    #[test]
    fn pick_episode_url_decimal_episodes() {
        let episodes = vec![
            ("12".to_string(), "https://site/ep12".to_string()),
            ("12.5".to_string(), "https://site/ep12_5".to_string()),
            ("13".to_string(), "https://site/ep13".to_string()),
        ];
        assert_eq!(
            pick_episode_url(&episodes, 12.5),
            Some("https://site/ep12_5".to_string())
        );
    }

    #[test]
    fn pick_episode_url_no_match() {
        let episodes = vec![
            ("1".to_string(), "https://site/ep1".to_string()),
            ("2".to_string(), "https://site/ep2".to_string()),
            ("OVA".to_string(), "https://site/ova".to_string()),
        ];
        assert_eq!(pick_episode_url(&episodes, 3.0), None);
    }

    #[test]
    fn pick_episode_url_empty_list() {
        let episodes: &[(String, String)] = &[];
        assert_eq!(pick_episode_url(episodes, 1.0), None);
    }
}
