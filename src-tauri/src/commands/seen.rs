use super::*;

/// Open an episode or info page in a visible, app-owned WebView2 window
/// (separate from the user's browser — see `AppWindowPlayer`). Does NOT mark
/// it seen — the user marks seen/unseen explicitly via `set_seen`.
///
/// `async` on purpose: creating a WebView2 window from a *synchronous* command
/// produced a blank window that closed itself (and an earlier main-thread-
/// dispatch attempt deadlocked the UI). Async commands run on Tauri's runtime,
/// the same context `scraper_engine` builds its windows from successfully.
#[tauri::command]
pub async fn open_episode(
    app: AppHandle,
    state: State<'_, AppState>,
    url: String,
    monitor: Option<String>,
    fullscreen: Option<bool>,
) -> Result<(), String> {
    let (eff_monitor, eff_fullscreen) = {
        let db = state.db.lock().unwrap();
        let mon = monitor.or_else(|| db.get_player_monitor().ok().flatten());
        let fs = fullscreen.unwrap_or_else(|| db.get_player_fullscreen().unwrap_or(false));
        (mon, fs)
    };

    let ep = Episode {
        id: 0,
        series_id: 0,
        number: String::new(),
        title: None,
        url,
        released_at: None,
        seen: false,
        download_path: None,
        download_status: None,
    };
    AppWindowPlayer
        .open_with_options(&app, &ep, eff_monitor.as_deref(), eff_fullscreen)
        .map_err(|e| e.to_string())
}

/// Lists all currently connected/available displays/monitors on the system.
#[tauri::command]
pub fn list_monitors(app: AppHandle) -> Result<Vec<crate::models::MonitorOption>, String> {
    let monitors = app.available_monitors().map_err(|e| e.to_string())?;
    let primary = app.primary_monitor().ok().flatten();
    let result = monitors
        .into_iter()
        .enumerate()
        .map(|(idx, m)| {
            let is_primary = primary
                .as_ref()
                .map(|p| p.name() == m.name() && p.position() == m.position())
                .unwrap_or(false);
            let name = m.name().cloned().unwrap_or_else(|| format!("Display {}", idx + 1));
            let id = m.name().cloned().unwrap_or_else(|| format!("index-{}", idx));
            crate::models::MonitorOption {
                id,
                name,
                index: idx,
                is_primary,
                width: m.size().width,
                height: m.size().height,
            }
        })
        .collect();
    Ok(result)
}

/// Retrieves stored player playback preferences (target monitor and fullscreen).
#[tauri::command]
pub fn get_player_settings(state: State<'_, AppState>) -> Result<crate::models::PlayerSettings, String> {
    let db = state.db.lock().unwrap();
    let monitor = db.get_player_monitor().map_err(|e| e.to_string())?;
    let fullscreen = db.get_player_fullscreen().map_err(|e| e.to_string())?;
    Ok(crate::models::PlayerSettings { monitor, fullscreen })
}

/// Persists player playback preferences (target monitor and fullscreen).
#[tauri::command]
pub fn set_player_settings(
    state: State<'_, AppState>,
    monitor: Option<String>,
    fullscreen: bool,
) -> Result<(), String> {
    let db = state.db.lock().unwrap();
    db.set_player_monitor(monitor.as_deref()).map_err(|e| e.to_string())?;
    db.set_player_fullscreen(fullscreen).map_err(|e| e.to_string())?;
    Ok(())
}


/// Mark an episode seen or unseen (persisted). When marking seen, any
/// downloaded video file for the episode is automatically deleted from disk.
#[tauri::command]
pub fn set_seen(state: State<'_, AppState>, episode_id: i64, seen: bool) -> Result<(), String> {
    let db = state.db.lock().unwrap();
    db.set_seen(episode_id, seen).map_err(|e| e.to_string())?;
    if seen {
        cleanup_downloads(&db, &[episode_id]);
    }
    Ok(())
}

/// Mark an episode seen/unseen, cascading to keep watching gap-free: marking
/// seen also marks every earlier episode of the series seen; marking unseen
/// also marks every later one unseen. When marking seen, downloaded video
/// files for all affected episodes are automatically deleted from disk.
#[tauri::command]
pub fn set_seen_cascade(
    state: State<'_, AppState>,
    series_id: i64,
    number: String,
    seen: bool,
) -> Result<(), String> {
    let db = state.db.lock().unwrap();
    let mut sids = db
        .canonical_series_ids(series_id)
        .unwrap_or_else(|_| vec![series_id]);
    if sids.is_empty() {
        sids.push(series_id);
    }
    // Collect the IDs that will flip to seen BEFORE the cascade runs, so we
    // know which downloads to clean up afterwards.
    let affected_ids = if seen {
        let mut ids = Vec::new();
        for &sid in &sids {
            if let Ok(ep_ids) = db.episode_ids_in_cascade(sid, &number, true) {
                ids.extend(ep_ids);
            }
        }
        ids
    } else {
        vec![]
    };
    db.set_seen_cascade_in(&sids, &number, seen)
        .map_err(|e| e.to_string())?;
    if seen && !affected_ids.is_empty() {
        cleanup_downloads(&db, &affected_ids);
    }
    Ok(())
}

/// Hard-delete a series row outright — used for "Eliminar del todo" on
/// discarded backlog rows, which never have episodes so this is always
/// safe there.
#[tauri::command]
pub fn delete_series(state: State<'_, AppState>, series_id: i64) -> Result<(), String> {
    let db = state.db.lock().unwrap();
    db.delete_series(series_id).map_err(|e| e.to_string())
}

/// Delete downloaded video files from disk and clear their DB columns.
/// Best-effort: a missing file is not an error (the column is still cleared
/// to avoid an orphaned "downloaded" state forever).
fn cleanup_downloads(db: &crate::db::Db, episode_ids: &[i64]) {
    match db.downloaded_paths(episode_ids) {
        Ok(paths) => {
            let mut cleaned = Vec::new();
            for (id, path) in &paths {
                match std::fs::remove_file(path) {
                    Ok(()) => cleaned.push(*id),
                    Err(e) => {
                        eprintln!("[download] failed to delete {path}: {e}");
                        // Clear DB anyway — a missing file is better than a
                        // perpetually-stuck "downloaded" badge.
                        cleaned.push(*id);
                    }
                }
            }
            if !cleaned.is_empty() {
                if let Err(e) = db.clear_download_columns(&cleaned) {
                    eprintln!("[download] failed to clear download columns: {e}");
                }
            }
        }
        Err(e) => eprintln!("[download] failed to query download paths: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleanup_downloads_deletes_file_and_clears_db() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
        let s = crate::models::Series {
            id: 0, slug: "test".into(), title: "Test".into(),
            url: "https://site/tv/test/".into(), cover_url: None, is_airing: false, followed: true, next_episode_at: None, site_episode_count: None,
        };
        let sid = db.upsert_series(src, &s).unwrap();

        let (ep_id, _) = db.insert_episode(&crate::models::Episode {
            id: 0, series_id: sid, number: "1".into(), title: None,
            url: "https://site/test-1".into(), released_at: None, seen: false,
            download_path: None, download_status: None,
        }).unwrap();

        // Create a fake downloaded file on disk
        let temp_file = std::env::temp_dir().join(format!("aot_seen_dl_test_{}.mp4", ep_id));
        std::fs::write(&temp_file, b"video-content").unwrap();
        assert!(temp_file.exists());

        db.set_download_status(ep_id, "downloaded", Some(temp_file.to_str().unwrap())).unwrap();

        // Call cleanup_downloads
        cleanup_downloads(&db, &[ep_id]);

        // Verify the file was removed from disk
        assert!(!temp_file.exists(), "Downloaded file should be deleted upon cleanup");

        // Verify the DB columns were cleared
        let ep = db.get_episode(ep_id).unwrap();
        assert!(ep.download_path.is_none());
        assert!(ep.download_status.is_none());
    }

    #[test]
    fn cascade_deletes_downloaded_files_up_to_number() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
        let s = crate::models::Series {
            id: 0, slug: "test-cascade".into(), title: "Test Cascade".into(),
            url: "https://site/tv/test-cascade/".into(), cover_url: None, is_airing: false, followed: true, next_episode_at: None, site_episode_count: None,
        };
        let sid = db.upsert_series(src, &s).unwrap();

        // Create 3 episodes: 1, 2, 3
        let mut ep_ids = Vec::new();
        let mut temp_files = Vec::new();
        for num in ["1", "2", "3"] {
            let (ep_id, _) = db.insert_episode(&crate::models::Episode {
                id: 0, series_id: sid, number: num.to_string(), title: None,
                url: format!("https://site/test-{}", num), released_at: None, seen: false,
                download_path: None, download_status: None,
            }).unwrap();
            ep_ids.push(ep_id);

            let temp_file = std::env::temp_dir().join(format!("aot_cascade_dl_test_{}_{}.mp4", sid, num));
            std::fs::write(&temp_file, b"content").unwrap();
            assert!(temp_file.exists());
            db.set_download_status(ep_id, "downloaded", Some(temp_file.to_str().unwrap())).unwrap();
            temp_files.push(temp_file);
        }

        // Collect affected IDs before cascade
        let affected_ids = db.episode_ids_in_cascade(sid, "3", true).unwrap();
        assert_eq!(affected_ids.len(), 3);
        db.set_seen_cascade(sid, "3", true).unwrap();
        cleanup_downloads(&db, &affected_ids);

        // All 3 files should be deleted from disk
        for file in &temp_files {
            assert!(!file.exists(), "File {:?} should have been deleted", file);
        }

        // All 3 episodes should have cleared download columns and seen=true
        for ep_id in &ep_ids {
            let ep = db.get_episode(*ep_id).unwrap();
            assert!(ep.seen);
            assert!(ep.download_path.is_none());
            assert!(ep.download_status.is_none());
        }
    }
}

