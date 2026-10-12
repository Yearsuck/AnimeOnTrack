use super::*;

/// Download an episode's video for offline viewing. This is a placeholder
/// implementation — full video-URL extraction from the streaming sites'
/// embedded players requires site-specific JavaScript evaluation (like
/// `scraper_engine`), which is a complex follow-up. For now this sets up
/// the DB bookkeeping and file structure so the auto-delete-on-seen path
/// and the UI are fully functional.
#[tauri::command]
pub async fn download_episode(
    _app: AppHandle,
    state: State<'_, AppState>,
    episode_id: i64,
) -> Result<(), String> {
    let (_url, _series_id) = {
        let db = state.db.lock().unwrap();
        let ep = db.get_episode(episode_id).map_err(|e| e.to_string())?;
        if ep.download_status.as_deref() == Some("downloading") {
            return Err("Already downloading this episode".into());
        }
        if ep.download_status.as_deref() == Some("downloaded") {
            return Err("Episode is already downloaded".into());
        }
        db.set_download_status(episode_id, "downloading", None)
            .map_err(|e| e.to_string())?;
        (ep.url, ep.series_id)
    };

    // TODO: Implement video URL extraction + streaming download.
    // The architecture is ready:
    //   1. Open episode page in a hidden WebView (like scraper_engine)
    //   2. Inject JS to find the <video> source or iframe embed URL
    //   3. Stream-download via reqwest with progress events:
    //      app.emit("download-progress", DownloadProgress { episode_id, percent })
    //   4. Save to <app_data_dir>/downloads/<series_id>/<episode_id>.mp4
    //   5. Update DB: set_download_status(episode_id, "downloaded", Some(path))
    //
    // For now, mark as failed so the UI reflects the real state.
    {
        let db = state.db.lock().unwrap();
        db.set_download_status(episode_id, "failed", None)
            .map_err(|e| e.to_string())?;
    }

    Err("Video download is not yet implemented — the auto-delete-on-seen infrastructure is ready.".into())
}

/// Cancel an in-progress download.
#[tauri::command]
pub fn cancel_download(
    state: State<'_, AppState>,
    episode_id: i64,
) -> Result<(), String> {
    let db = state.db.lock().unwrap();
    db.set_download_status(episode_id, "failed", None)
        .map_err(|e| e.to_string())?;
    // TODO: Cancel the actual reqwest download task.
    Ok(())
}

/// Delete a downloaded episode's file and clear its DB state.
#[tauri::command]
pub fn delete_download(
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
pub fn get_downloads_size(
    state: State<'_, AppState>,
) -> Result<u64, String> {
    let db = state.db.lock().unwrap();
    db.downloads_disk_size().map_err(|e| e.to_string())
}

/// Delete ALL downloaded files and clear their DB columns.
#[tauri::command]
pub fn clear_all_downloads(
    state: State<'_, AppState>,
) -> Result<usize, String> {
    let db = state.db.lock().unwrap();
    db.clear_all_downloads().map_err(|e| e.to_string())
}
