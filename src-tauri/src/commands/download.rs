use super::*;
use crate::download_sources::{candidates, parse_tioanime_servers, resolve_yourupload, Media};
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
/// Only TioAnime episode pages are understood so far (the other sites need the
/// same research as `download_sources`), and only YourUpload mirrors can be
/// resolved to a plain mp4; anything else reports a clear reason.
#[tauri::command]
pub async fn download_episode(
    app: AppHandle,
    state: State<'_, AppState>,
    episode_id: i64,
) -> Result<(), String> {
    {
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
    emit(&app, episode_id, "downloading", Some(0), None);

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
    let dest = downloader::episode_file_path(&dir, series_id, episode_id);

    let media = resolve_media(app, &episode_url).await?;
    if cancel.load(Ordering::SeqCst) {
        return Err(DlError::Cancelled);
    }

    let mut last_pct: i16 = -1;
    let app_for_progress = app.clone();
    downloader::fetch_to_file(&media, &dest, cancel, move |done, total| {
        let Some(total) = total.filter(|t| *t > 0) else { return };
        let pct = ((done.saturating_mul(100)) / total).min(100) as i16;
        if pct != last_pct {
            last_pct = pct;
            emit(&app_for_progress, episode_id, "downloading", Some(pct as u8), None);
        }
    })
    .await?;
    Ok(dest.to_string_lossy().into_owned())
}

/// Episode page -> list of mirrors -> first mirror that resolves to a plain mp4.
async fn resolve_media(app: &AppHandle, episode_url: &str) -> Result<Media, DlError> {
    if !is_safe_external_url(episode_url) {
        return Err(DlError::Net("the episode link is not a public web address".into()));
    }
    let host = url::Url::parse(episode_url)
        .ok()
        .and_then(|u| u.host_str().map(|h| h.to_ascii_lowercase()))
        .unwrap_or_default();
    if !(host == "tioanime.com" || host.ends_with(".tioanime.com")) {
        return Err(DlError::Net(format!("downloads from {host} are not supported yet")));
    }

    // The episode page sits behind Cloudflare, so it goes through the browser
    // engine like every other page fetch.
    let page = fetch_html_with_script(app, episode_url, None)
        .await
        .map_err(|e| DlError::Net(format!("could not load the episode page: {e}")))?;
    let servers = parse_tioanime_servers(&page.html);
    let options = candidates(&servers);
    if options.is_empty() {
        let names: Vec<&str> = servers.iter().map(|s| s.name.as_str()).collect();
        return Err(DlError::Net(format!(
            "no supported mirror on this episode (found: {})",
            if names.is_empty() { "none".to_string() } else { names.join(", ") }
        )));
    }

    let mut last_err = String::from("no mirror could be resolved");
    for server in options {
        if !is_safe_external_url(&server.url) {
            continue;
        }
        let resp = downloader::client()
            .get(&server.url)
            .header(reqwest::header::USER_AGENT, downloader::browser_user_agent())
            .header(reqwest::header::REFERER, episode_url)
            .timeout(std::time::Duration::from_secs(20))
            .send()
            .await;
        let html = match resp {
            Ok(r) if r.status().is_success() => r.text().await.unwrap_or_default(),
            Ok(r) => {
                last_err = format!("{} answered HTTP {}", server.name, r.status());
                continue;
            }
            Err(e) => {
                last_err = format!("{}: {e}", server.name);
                continue;
            }
        };
        match resolve_yourupload(&html, &server.url) {
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
