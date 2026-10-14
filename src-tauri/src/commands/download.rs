use super::*;
use crate::download_sources::{
    candidates, mytsumi_direct_media, parse_animeyt_container_url, parse_animeflv_servers, parse_jkanime_servers, parse_mytsumi_tabs,
    parse_tioanime_servers, resolve_from_page, supported_host, voe_redirect_target, Media, Server,
};
use crate::hls::{parse_gogoanime_player_url, parse_megaplay_playlist_url, parse_wrapper_iframe};
use crate::downloader::{self, DlError};
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
    let episodes = {
        let db = state.db.lock().unwrap();
        db.list_series_episodes(series_id).map_err(|e| e.to_string())?
    };
    let mut batch = DownloadBatch { queued: 0, unsupported: 0, already: 0 };
    for ep in episodes.iter().filter(|e| !e.seen) {
        match ep.download_status.as_deref() {
            Some("downloading") | Some("downloaded") => batch.already += 1,
            _ if !is_supported_episode_url(&ep.url) => batch.unsupported += 1,
            _ => {
                if enqueue(&app, ep.id).is_ok() {
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

async fn run_download(app: &AppHandle, episode_id: i64, cancel: &Arc<AtomicBool>) -> Result<String, DlError> {
    let _slot = SLOT.acquire().await.map_err(|e| DlError::Net(e.to_string()))?;
    if cancel.load(Ordering::SeqCst) {
        return Err(DlError::Cancelled);
    }

    let (episode_url, series_id) = {
        let state = app.state::<AppState>();
        let db = state.db.lock().unwrap();
        let ep = db.get_episode(episode_id).map_err(|e| DlError::Io(e.to_string()))?;
        (ep.url, ep.series_id)
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

    let media = resolve_media(app, &episode_url).await?;
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
    let is_hls = url::Url::parse(&media.url)
        .map(|u| u.path().to_ascii_lowercase().ends_with(".m3u8"))
        .unwrap_or(false);
    let final_file = if is_hls {
        downloader::fetch_hls_to_file(&media.url, &media.referer, &base, cancel, progress).await?
    } else {
        downloader::fetch_to_file(&media, &base, cancel, progress).await?
    };
    Ok(final_file.to_string_lossy().into_owned())
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

/// GogoAnime: episode page -> its player wrapper -> the megaplay embed -> an
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
    let playlist = parse_megaplay_playlist_url(&embed)
        .ok_or_else(|| DlError::Net("the video frame has no stream".into()))?;
    if !is_safe_external_url(&playlist) {
        return Err(DlError::Net("the stream is not a public web address".into()));
    }
    Ok(Media { url: playlist, referer: "https://megaplay.su/".to_string(), host: "hls" })
}

/// Episode page -> list of mirrors -> first mirror that resolves to a plain file.
async fn resolve_media(app: &AppHandle, episode_url: &str) -> Result<Media, DlError> {
    if !is_safe_external_url(episode_url) {
        return Err(DlError::Net("the episode link is not a public web address".into()));
    }
    let Some(site) = site_of(episode_url) else {
        let host = url::Url::parse(episode_url)
            .ok()
            .and_then(|u| u.host_str().map(str::to_owned))
            .unwrap_or_default();
        return Err(DlError::Net(format!("downloads from {host} are not supported yet")));
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

    let mut options = candidates(&servers);
    // Development aid: `AOT_DOWNLOAD_PREFER=<host id>` (e.g. `voe`, `mixdrop`) tries
    // that host's mirrors first, so one extractor can be exercised on a live page.
    if let Some(prefer) = std::env::var("AOT_DOWNLOAD_PREFER").ok().filter(|p| !p.trim().is_empty()) {
        options.sort_by_key(|s| supported_host(s) != Some(prefer.trim().to_ascii_lowercase().as_str()));
    }
    if options.is_empty() {
        let names: Vec<&str> = servers.iter().map(|s| s.name.as_str()).collect();
        return Err(DlError::Net(format!(
            "no supported mirror on this episode (found: {})",
            if names.is_empty() { "none".to_string() } else { names.join(", ") }
        )));
    }

    let mut last_err = String::from("no mirror could be resolved");
    'servers: for server in options {
        let Some(host) = supported_host(&server) else { continue };
        let (mut page_url, mut html) = match fetch_text(&server.url, episode_url).await {
            Ok(p) => p,
            Err(e) => {
                last_err = format!("{}: {e}", server.name);
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
                        last_err = format!("{}: {e}", server.name);
                        continue 'servers;
                    }
                }
            }
        }
        match resolve_from_page(host, &html, &page_url) {
            Some(media) if is_safe_external_url(&media.url) => return Ok(media),
            _ => last_err = format!("{} has no downloadable file", server.name),
        }
    }
    Err(DlError::Net(last_err))
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
}
