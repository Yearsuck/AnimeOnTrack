use super::*;
use rusqlite::OptionalExtension;

/// Open an episode or info page in a visible, app-owned WebView2 window
/// (separate from the user's browser — see `AppWindowPlayer`); on Android the
/// system browser instead (`BrowserPlayer`). Does NOT mark
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
    let (eff_monitor, eff_fullscreen, eff_clean) = {
        state.with_db(|db| {
            let mon = monitor.or_else(|| db.get_player_monitor().ok().flatten());
            let fs = fullscreen.unwrap_or_else(|| db.get_player_fullscreen().unwrap_or(false));
            let clean = db.get_player_clean().unwrap_or(true);
            Ok((mon, fs, clean))
        })?
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
    // On a phone an app window is the whole screen: the scraped page covered the
    // app with no close button and the back key did nothing (seen on the
    // emulator), so the only way out was killing the app. The system browser has
    // its own back navigation.
    #[cfg(mobile)]
    let player = crate::player::BrowserPlayer;
    #[cfg(desktop)]
    let player = AppWindowPlayer;
    player
        .open_with_options(&app, &ep, eff_monitor.as_deref(), eff_fullscreen, eff_clean)
        .map_err(|e| e.to_string())
}

/// Lists all currently connected/available displays/monitors on the system.
#[tauri::command]
pub async fn list_monitors(app: AppHandle) -> Result<Vec<crate::models::MonitorOption>, String> {
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
pub async fn get_player_settings(state: State<'_, AppState>) -> Result<crate::models::PlayerSettings, String> {
    Ok(state.with_db(|db| {
        let monitor = db.get_player_monitor()?;
        let fullscreen = db.get_player_fullscreen()?;
        let clean = db.get_player_clean()?;
        let adblock = db.get_player_adblock()?;
        Ok(crate::models::PlayerSettings { monitor, fullscreen, clean, adblock })
    })?)
}

/// Persists player playback preferences (target monitor and fullscreen).
#[tauri::command]
pub async fn set_player_settings(
    state: State<'_, AppState>,
    monitor: Option<String>,
    fullscreen: bool,
    clean: bool,
    adblock: Option<bool>,
) -> Result<(), String> {
    Ok(state.with_db(|db| {
        db.set_player_monitor(monitor.as_deref())?;
        db.set_player_fullscreen(fullscreen)?;
        db.set_player_clean(clean)?;
        if let Some(adblock) = adblock {
            db.set_player_adblock(adblock)?;
            crate::ad_block::set_engine_enabled(adblock);
        }
        Ok(())
    })?)
}

/// A streaming site source for an episode.
#[derive(serde::Serialize, Clone, Debug, PartialEq)]
pub struct EpisodeSource {
    pub episode_id: i64,
    pub site_id: String,
    pub site_name: String,
    pub url: String,
}

/// Helper to get a human-friendly display name for a site slug.
fn site_display_name(site_id: &str) -> String {
    crate::adapter::all_sites()
        .iter()
        .find(|s| s.id == site_id)
        .map(|s| s.name.to_string())
        .unwrap_or_else(|| site_id.to_string())
}

/// Pure ordering of episode sources:
/// 1. De-duplicates by `site_id` (keeping the first occurrence).
/// 2. Sorts:
///    - Active site first (if present).
///    - Then by position in the configured `priority` list.
///    - Then unknown sites (not in `priority`) ordered alphabetically by `site_id`.
///
/// Stable sort.
pub(crate) fn order_sources(
    list: Vec<EpisodeSource>,
    active: &str,
    priority: &[String],
) -> Vec<EpisodeSource> {
    let mut deduped = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for item in list {
        if seen.insert(item.site_id.clone()) {
            deduped.push(item);
        }
    }

    fn key<'a>(s: &'a EpisodeSource, active: &str, priority: &[String]) -> (u8, usize, &'a str) {
        if !active.is_empty() && s.site_id == active {
            (0, 0, "")
        } else if let Some(pos) = priority.iter().position(|p| p == &s.site_id) {
            (1, pos, "")
        } else {
            (2, 0, s.site_id.as_str())
        }
    }
    deduped.sort_by(|a, b| key(a, active, priority).cmp(&key(b, active, priority)));

    deduped
}

/// Assemble sources for `episode_id` by querying its row from the database
/// (its url and site_id) plus any sibling episodes in the same canonical group.
/// Returns an empty Vec if the episode id does not exist.
pub fn assemble_episode_sources(db: &crate::db::Db, episode_id: i64) -> Result<Vec<EpisodeSource>, String> {
    let clicked: Option<(String, String)> = db
        .conn
        .query_row(
            "SELECT COALESCE(so.site_id, ''), e.url
             FROM episodes e
             JOIN series s ON s.id = e.series_id
             LEFT JOIN sources so ON so.id = s.source_id
             WHERE e.id = ?1",
            [episode_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(|e| e.to_string())?;

    let (clicked_site_id, clicked_url) = match clicked {
        Some(v) => v,
        None => return Ok(vec![]),
    };

    let mut sources = Vec::new();
    sources.push(EpisodeSource {
        episode_id,
        site_id: clicked_site_id.clone(),
        site_name: site_display_name(&clicked_site_id),
        url: clicked_url,
    });

    let siblings = db.sibling_episodes(episode_id).map_err(|e| e.to_string())?;
    for sib in siblings.into_iter().filter(|s| !s.site_id.is_empty()) {
        sources.push(EpisodeSource {
            episode_id: sib.episode_id,
            site_id: sib.site_id.clone(),
            site_name: site_display_name(&sib.site_id),
            url: sib.url,
        });
    }

    Ok(sources)
}

/// Retrieves all compatible site sources for an episode, with the active site
/// first, followed by priority order.
#[tauri::command]
pub async fn episode_sources(
    state: State<'_, AppState>,
    episode_id: i64,
) -> Result<Vec<EpisodeSource>, String> {
    let active = state.active_site_id.lock().unwrap().clone();
    let found = state.with_db(|db| {
        let sources = assemble_episode_sources(db, episode_id).map_err(crate::error::AppError::Other)?;
        if sources.is_empty() {
            return Ok(None);
        }
        let known: Vec<&str> = crate::adapter::all_sites().iter().map(|s| s.id).collect();
        let priority = db.get_site_priority(&known)?;
        Ok(Some((sources, priority)))
    })?;
    let Some((sources, priority)) = found else {
        return Ok(vec![]);
    };
    Ok(order_sources(sources, &active, &priority))
}


/// Mark an episode seen or unseen (persisted). When marking seen, any
/// downloaded video file for the episode is automatically deleted from disk
/// if auto-delete is enabled in settings.
#[tauri::command]
pub async fn set_seen(
    app: AppHandle,
    state: State<'_, AppState>,
    episode_id: i64,
    seen: bool,
) -> Result<(), String> {
    let (should_cleanup, downloads_dir) = {
        state.with_db(|db| {
            db.set_seen(episode_id, seen)?;
            let auto_delete = db.get_auto_delete_seen_downloads().unwrap_or(true);
            let dir = app.path().app_data_dir().ok().map(|d| d.join("downloads"));
            Ok((seen && auto_delete, dir))
        })?
    };
    if should_cleanup {
        state.with_db(|db| {
            cleanup_downloads(db, &[episode_id], downloads_dir.as_deref());
            Ok(())
        })?;
    }
    Ok(())
}

/// Mark an episode seen/unseen, cascading to keep watching gap-free: marking
/// seen also marks every earlier episode of the series seen; marking unseen
/// also marks every later one unseen. When marking seen, downloaded video
/// files for all affected episodes are automatically deleted from disk
/// if auto-delete is enabled in settings.
#[tauri::command]
pub async fn set_seen_cascade(
    app: AppHandle,
    state: State<'_, AppState>,
    series_id: i64,
    number: String,
    seen: bool,
) -> Result<(), String> {
    let (affected_ids, auto_delete, downloads_dir) = state.with_db(|db| {
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
            let now = chrono::Utc::now().timestamp();
            let clicked_align = db.row_align(series_id, now).ok();
            let canonical = crate::db::parse_ep_number(&number)
                .and_then(|n| clicked_align.as_ref()?.to_canonical(n));
            if let Some(c) = canonical {
                for &sid in &sids {
                    if let Ok(align) = db.row_align(sid, now) {
                        let num_str = crate::db::episode_align::format_ep_number(align.to_row(c));
                        if let Ok(ep_ids) = db.episode_ids_in_cascade(sid, &num_str, true) {
                            ids.extend(ep_ids);
                        }
                    }
                }
            } else if let Ok(ep_ids) = db.episode_ids_in_cascade(series_id, &number, true) {
                ids.extend(ep_ids);
            }
            ids
        } else {
            vec![]
        };
        db.set_seen_cascade_in(series_id, &sids, &number, seen)?;
        let auto_delete = db.get_auto_delete_seen_downloads().unwrap_or(true);
        let downloads_dir = app.path().app_data_dir().ok().map(|d| d.join("downloads"));
        Ok((affected_ids, auto_delete, downloads_dir))
    })?;

    if seen && auto_delete && !affected_ids.is_empty() {
        state.with_db(|db| {
            cleanup_downloads(db, &affected_ids, downloads_dir.as_deref());
            Ok(())
        })?;
    }
    Ok(())
}

/// Hard-delete a series row outright — used for "Eliminar del todo" on
/// discarded backlog rows, which never have episodes so this is always
/// safe there. Also cleans up any downloaded files and the series directory.
#[tauri::command]
pub async fn delete_series(
    app: AppHandle,
    state: State<'_, AppState>,
    series_id: i64,
) -> Result<(), String> {
    let running: Vec<i64> = {
        state.with_db(|db| {
            Ok(db
                .list_series_episodes(series_id)?
                .into_iter()
                .filter(|e| e.download_status.as_deref() == Some("downloading"))
                .map(|e| e.id)
                .collect())
        })?
    };
    // A running download would otherwise finish into a folder of a series
    // that no longer exists, leaving an orphan behind.
    crate::commands::download::cancel_active(&running);
    let (paths, downloads_dir) = {
        state.with_db(|db| {
            let paths = db.series_downloaded_paths(series_id).unwrap_or_default();
            db.delete_series(series_id)?;
            let dir = app.path().app_data_dir().ok().map(|d| d.join("downloads"));
            Ok((paths, dir))
        })?
    };
    if let Some(ref dir) = downloads_dir {
        crate::commands::storage::delete_series_files_and_dir(dir, series_id, &paths);
    }
    Ok(())
}

/// Delete downloaded video files from disk and clear their DB columns.
/// Best-effort: a missing file is not an error (the column is still cleared
/// to avoid an orphaned "downloaded" state forever). Also removes parent
/// series folder if it becomes empty.
pub(crate) fn cleanup_downloads(
    db: &crate::db::Db,
    episode_ids: &[i64],
    downloads_dir: Option<&std::path::Path>,
) {
    match db.downloaded_paths(episode_ids) {
        Ok(paths) => {
            let mut cleaned = Vec::new();
            for (id, path) in &paths {
                let p = std::path::Path::new(path);
                let can_delete = match downloads_dir {
                    Some(dir) => crate::download_safety::is_safe_file_in_dir(dir, p),
                    None => p.is_file(),
                };
                if can_delete {
                    match std::fs::remove_file(p) {
                        Ok(()) => {
                            cleaned.push(*id);
                            if let Some(parent) = p.parent() {
                                if let Some(dir) = downloads_dir {
                                    crate::download_safety::remove_empty_dir_in_dir(dir, parent);
                                } else {
                                    let _ = std::fs::remove_dir(parent);
                                }
                            }
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                            cleaned.push(*id);
                        }
                        Err(e) => {
                            eprintln!("[download] failed to delete {path}: {e}");
                        }
                    }
                } else if !p.exists() {
                    cleaned.push(*id);
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
        cleanup_downloads(&db, &[ep_id], None);

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
        cleanup_downloads(&db, &affected_ids, None);

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

    #[test]
    fn cleanup_downloads_removes_series_dir_when_it_becomes_empty() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
        let sid = db.upsert_series(src, &crate::models::Series {
            id: 0, slug: "test-dir-clean".into(), title: "Test Dir Clean".into(),
            url: "https://site/tv/test-dir-clean/".into(), cover_url: None, is_airing: false, followed: true, next_episode_at: None, site_episode_count: None,
        }).unwrap();

        let (ep_id, _) = db.insert_episode(&crate::models::Episode {
            id: 0, series_id: sid, number: "1".into(), title: None,
            url: "https://site/test-1".into(), released_at: None, seen: false,
            download_path: None, download_status: None,
        }).unwrap();

        let temp_root = std::env::temp_dir().join(format!("aot_dl_dir_test_{}", sid));
        let series_dir = temp_root.join(sid.to_string());
        std::fs::create_dir_all(&series_dir).unwrap();
        let ep_file = series_dir.join(format!("{}.mp4", ep_id));
        std::fs::write(&ep_file, b"video").unwrap();

        db.set_download_status(ep_id, "downloaded", Some(ep_file.to_str().unwrap())).unwrap();

        cleanup_downloads(&db, &[ep_id], Some(&temp_root));

        assert!(!ep_file.exists());
        assert!(!series_dir.exists(), "Series folder should be removed when empty");
        let _ = std::fs::remove_dir_all(&temp_root);
    }

    #[test]
    fn cleanup_downloads_preserves_series_dir_when_other_episodes_remain() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
        let sid = db.upsert_series(src, &crate::models::Series {
            id: 0, slug: "test-dir-keep".into(), title: "Test Dir Keep".into(),
            url: "https://site/tv/test-dir-keep/".into(), cover_url: None, is_airing: false, followed: true, next_episode_at: None, site_episode_count: None,
        }).unwrap();

        let (ep1, _) = db.insert_episode(&crate::models::Episode {
            id: 0, series_id: sid, number: "1".into(), title: None,
            url: "https://site/test-1".into(), released_at: None, seen: false,
            download_path: None, download_status: None,
        }).unwrap();
        let (ep2, _) = db.insert_episode(&crate::models::Episode {
            id: 0, series_id: sid, number: "2".into(), title: None,
            url: "https://site/test-2".into(), released_at: None, seen: false,
            download_path: None, download_status: None,
        }).unwrap();

        let temp_root = std::env::temp_dir().join(format!("aot_dl_dir_keep_{}", sid));
        let series_dir = temp_root.join(sid.to_string());
        std::fs::create_dir_all(&series_dir).unwrap();
        let file1 = series_dir.join(format!("{}.mp4", ep1));
        let file2 = series_dir.join(format!("{}.mp4", ep2));
        std::fs::write(&file1, b"vid1").unwrap();
        std::fs::write(&file2, b"vid2").unwrap();

        db.set_download_status(ep1, "downloaded", Some(file1.to_str().unwrap())).unwrap();
        db.set_download_status(ep2, "downloaded", Some(file2.to_str().unwrap())).unwrap();

        // Cleanup only ep1
        cleanup_downloads(&db, &[ep1], Some(&temp_root));

        assert!(!file1.exists());
        assert!(file2.exists());
        assert!(series_dir.exists(), "Series folder should remain when other files are present");
        let _ = std::fs::remove_dir_all(&temp_root);
    }

    #[test]
    fn order_sources_active_first_and_priority() {
        let list = vec![
            EpisodeSource { episode_id: 1, site_id: "tioanime".into(), site_name: "TioAnime".into(), url: "u1".into() },
            EpisodeSource { episode_id: 2, site_id: "animeflv".into(), site_name: "AnimeFLV".into(), url: "u2".into() },
            EpisodeSource { episode_id: 3, site_id: "animeytx".into(), site_name: "AnimeYT".into(), url: "u3".into() },
        ];
        let priority = vec!["animeytx".into(), "tioanime".into(), "animeflv".into()];
        let ordered = order_sources(list, "animeflv", &priority);
        let ids: Vec<&str> = ordered.iter().map(|s| s.site_id.as_str()).collect();
        assert_eq!(ids, vec!["animeflv", "animeytx", "tioanime"]);
    }

    #[test]
    fn order_sources_active_absent_pure_priority() {
        let list = vec![
            EpisodeSource { episode_id: 1, site_id: "tioanime".into(), site_name: "TioAnime".into(), url: "u1".into() },
            EpisodeSource { episode_id: 2, site_id: "animeytx".into(), site_name: "AnimeYT".into(), url: "u2".into() },
        ];
        let priority = vec!["animeytx".into(), "tioanime".into(), "animeflv".into()];
        let ordered = order_sources(list, "jkanime", &priority);
        let ids: Vec<&str> = ordered.iter().map(|s| s.site_id.as_str()).collect();
        assert_eq!(ids, vec!["animeytx", "tioanime"]);
    }

    #[test]
    fn order_sources_deduplicates_by_site_id_keeping_first() {
        let list = vec![
            EpisodeSource { episode_id: 1, site_id: "tioanime".into(), site_name: "TioAnime 1".into(), url: "u1".into() },
            EpisodeSource { episode_id: 2, site_id: "tioanime".into(), site_name: "TioAnime 2".into(), url: "u2".into() },
            EpisodeSource { episode_id: 3, site_id: "animeflv".into(), site_name: "AnimeFLV".into(), url: "u3".into() },
        ];
        let priority = vec!["tioanime".into(), "animeflv".into()];
        let ordered = order_sources(list, "tioanime", &priority);
        assert_eq!(ordered.len(), 2);
        assert_eq!(ordered[0].episode_id, 1);
        assert_eq!(ordered[0].site_id, "tioanime");
        assert_eq!(ordered[1].episode_id, 3);
        assert_eq!(ordered[1].site_id, "animeflv");
    }

    #[test]
    fn order_sources_unknown_sites_after_known_ordered_by_site_id() {
        let list = vec![
            EpisodeSource { episode_id: 1, site_id: "zzz_unknown".into(), site_name: "ZZZ".into(), url: "u1".into() },
            EpisodeSource { episode_id: 2, site_id: "tioanime".into(), site_name: "TioAnime".into(), url: "u2".into() },
            EpisodeSource { episode_id: 3, site_id: "aaa_unknown".into(), site_name: "AAA".into(), url: "u3".into() },
        ];
        let priority = vec!["tioanime".into()];
        let ordered = order_sources(list, "none", &priority);
        let ids: Vec<&str> = ordered.iter().map(|s| s.site_id.as_str()).collect();
        assert_eq!(ids, vec!["tioanime", "aaa_unknown", "zzz_unknown"]);
    }

    #[test]
    fn assemble_episode_sources_includes_clicked_and_siblings() {
        let db = Db::open(":memory:").unwrap();
        let src_a = db.upsert_source("AnimeYT", "https://animeyt.cc", "animeytx").unwrap();
        let src_b = db.upsert_source("TioAnime", "https://tioanime.com", "tioanime").unwrap();

        let s_a = crate::models::Series {
            id: 0, slug: "frieren-yt".into(), title: "Sousou no Frieren".into(),
            url: "https://animeyt.cc/tv/frieren/".into(), cover_url: None, is_airing: true, followed: true, next_episode_at: None, site_episode_count: None,
        };
        let sid_a = db.upsert_series(src_a, &s_a).unwrap();

        let s_b = crate::models::Series {
            id: 0, slug: "frieren-tio".into(), title: "Sousou no Frieren".into(),
            url: "https://tioanime.com/anime/frieren".into(), cover_url: None, is_airing: true, followed: true, next_episode_at: None, site_episode_count: None,
        };
        let sid_b = db.upsert_series(src_b, &s_b).unwrap();

        // Link both series to the same anilist_id
        db.conn.execute("UPDATE series SET anilist_id = 154587 WHERE id IN (?1, ?2)", [sid_a, sid_b]).unwrap();

        let (ep_a, _) = db.insert_episode(&crate::models::Episode {
            id: 0, series_id: sid_a, number: "3".into(), title: None,
            url: "https://animeyt.cc/tv/frieren-3/".into(), released_at: None, seen: false,
            download_path: None, download_status: None,
        }).unwrap();

        let (ep_b, _) = db.insert_episode(&crate::models::Episode {
            id: 0, series_id: sid_b, number: "3".into(), title: None,
            url: "https://tioanime.com/ver/frieren-3".into(), released_at: None, seen: false,
            download_path: None, download_status: None,
        }).unwrap();

        let sources = assemble_episode_sources(&db, ep_a).unwrap();
        assert_eq!(sources.len(), 2);
        assert_eq!(sources[0].episode_id, ep_a);
        assert_eq!(sources[0].site_id, "animeytx");
        assert_eq!(sources[0].site_name, "AnimeYT");
        assert_eq!(sources[0].url, "https://animeyt.cc/tv/frieren-3/");

        assert_eq!(sources[1].episode_id, ep_b);
        assert_eq!(sources[1].site_id, "tioanime");
        assert_eq!(sources[1].site_name, "TioAnime");
        assert_eq!(sources[1].url, "https://tioanime.com/ver/frieren-3");

        // Unknown episode id returns empty vec
        let unknown = assemble_episode_sources(&db, 99999).unwrap();
        assert!(unknown.is_empty());
    }
}

