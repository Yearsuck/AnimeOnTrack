mod adapter;
mod anilist;
mod backup;
mod commands;
mod dates;
mod db;
mod diff;
mod genres;
mod html_cache;
mod matching;
mod models;
mod player;
mod recommend;
mod scraper_engine;
mod swipe;

use commands::AppState;
use std::sync::Mutex;
use tauri::Manager;

/// The app-data directory name used before 0.5.5, when the Tauri identifier
/// still carried the author's own name (`com.ernes.aot-scaffold`).
const LEGACY_APP_DATA_DIR: &str = "com.ernes.aot-scaffold";

/// One-time carry-over of the database from the pre-0.5.5 app-data directory.
///
/// Changing the Tauri identifier moves `app_data_dir()`, which would
/// otherwise strand an existing install's whole library — the exact reason
/// the identifier had been pinned until now (see CLAUDE.md). This copies the
/// old database across the first time the new location is empty.
///
/// Deliberately a **copy, not a move**: the old directory is left untouched
/// so a mistake here is recoverable by simply reinstalling the old build,
/// and so the two installs (which Windows treats as separate apps, since the
/// identifier is part of the product code) can't corrupt each other by
/// sharing one file. Does nothing when the new location already has a
/// database, so it never overwrites real data on later launches — including
/// the case where the user has since used the app and the *old* copy is now
/// the stale one.
fn migrate_legacy_app_data(new_dir: &std::path::Path) {
    let db_name = "animeontrack.sqlite";
    if new_dir.join(db_name).exists() {
        return;
    }
    // The legacy directory is a sibling of the new one — same parent
    // (%APPDATA% on Windows), different identifier-derived name.
    let Some(legacy_dir) = new_dir.parent().map(|p| p.join(LEGACY_APP_DATA_DIR)) else {
        return;
    };
    let legacy_db = legacy_dir.join(db_name);
    if !legacy_db.exists() {
        return;
    }
    match std::fs::copy(&legacy_db, new_dir.join(db_name)) {
        Ok(bytes) => eprintln!("[migrate] carried {bytes} bytes over from {LEGACY_APP_DATA_DIR}"),
        // Non-fatal: the app still opens, just with an empty library, and
        // the legacy file is still sitting there to retry or recover from.
        Err(e) => eprintln!("[migrate] could not carry over the legacy database: {e}"),
    }
}

/// Append every panic (message + location) to `panic.log` next to the
/// database, on top of the default stderr behaviour.
///
/// A release build is a `windows_subsystem = "windows"` binary with no
/// console, so a panic message goes nowhere. That matters far more here than
/// in a normal Rust program: synchronous `#[tauri::command]`s run on the main
/// thread, invoked from the WebView2/COM callback, and a panic there cannot
/// unwind across the FFI boundary — the runtime aborts the process instead
/// (Windows exception `0xC0000409`, `FAST_FAIL_FATAL_APP_EXIT`). To the user
/// that is "the app just closed", with nothing to go on but a crash dump.
///
/// This is diagnosis, not a fix: the panic still aborts. It only means the
/// next one names its own file and line instead of costing a dump analysis.
fn install_panic_log(dir: &std::path::Path) {
    let log_path = dir.join("panic.log");
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        use std::io::Write;
        let when = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let location = info
            .location()
            .map(|l| format!("{}:{}", l.file(), l.line()))
            .unwrap_or_else(|| "unknown location".to_string());
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&log_path) {
            let _ = writeln!(f, "[{when}] panic at {location}: {info}");
        }
        previous(info);
    }));
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let dir = app.path().app_data_dir().expect("app data dir");
            std::fs::create_dir_all(&dir).ok();
            // Before anything that can panic below.
            install_panic_log(&dir);
            // Carry the database over from the pre-0.5.5 app-data directory,
            // which was keyed on the old `com.ernes.aot-scaffold` identifier.
            // Runs before everything below, so the rest of startup sees a
            // populated directory exactly as if it had always been here.
            migrate_legacy_app_data(&dir);
            // If a validated restore was staged by `restore_latest` on the
            // previous run, swap it in now, before the DB connection is
            // opened (Windows won't let us touch the file while it's held
            // open). Never leaves the app unopenable — any inconsistency
            // just clears the marker and falls through to the existing DB.
            backup::apply_pending_restore(&dir);
            let db_path = dir.join("animeontrack.sqlite");
            let db = db::Db::open(db_path.to_str().unwrap()).expect("open db");
            // Restore the active site from settings (installs that predate
            // multi-site support have no such key — default to the one site
            // that ever existed before this), then the most recent source
            // row tagged with that site, if that site has ever been scanned.
            let active_site_id = db
                .get_setting("active_site_id")
                .ok()
                .flatten()
                .unwrap_or_else(|| "animeytx".to_string());
            let source_id: Option<i64> = db.get_source_id_for_site(&active_site_id).ok().flatten();
            app.manage(AppState {
                db: Mutex::new(db),
                source_id: Mutex::new(source_id),
                active_site_id: Mutex::new(active_site_id),
                swipe_buffer: Mutex::new(std::collections::HashMap::new()),
                swipe_last_page: Mutex::new(std::collections::HashMap::new()),
                swipe_served: Mutex::new(std::collections::HashMap::new()),
                swipe_history: Mutex::new(std::collections::VecDeque::new()),
                html_cache: Mutex::new(html_cache::HtmlCache::default()),
                sticky_mirror: Mutex::new(std::collections::HashMap::new()),
                library_import_running: std::sync::atomic::AtomicBool::new(false),
                episode_backfill_running: std::sync::atomic::AtomicBool::new(false),
            });

            // Opportunistic startup cloud backup: silently does nothing
            // unless Google credentials are configured, Drive is connected,
            // and the throttle (24h + changed signature) says it's due.
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                let _ = commands::auto_backup_if_due(handle).await;
            });

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::scan_airing,
            commands::list_airing,
            commands::list_airing_season,
            commands::list_library,
            commands::get_genre_stats,
            commands::get_type_stats,
            commands::get_watch_summary,
            commands::get_watch_insights,
            commands::get_avg_completion_days,
            commands::get_stats_graph,
            commands::backfill_genres,
            commands::set_followed,
            commands::refresh,
            commands::list_pending,
            commands::pending_count,
            commands::open_episode,
            commands::set_seen,
            commands::set_seen_cascade,
            commands::list_episodes,
            commands::rescan_airing,
            commands::get_mirrors,
            commands::set_mirrors,
            commands::list_sites,
            commands::get_active_site,
            commands::set_active_site,
            commands::discover_swipe_card,
            commands::decide_swipe,
            commands::start_watching,
            commands::undo_last_swipe,
            commands::list_backlog,
            commands::list_watched_externally,
            commands::promote_discarded,
            commands::delete_series,
            commands::set_backlog_status,
            commands::reclassify_series,
            commands::get_series_genres,
            commands::get_top_genres,
            commands::get_anime_catalog,
            commands::get_catalog_facets,
            commands::sync_anime_catalog,
            commands::maybe_sync_catalog_incremental,
            commands::backfill_catalog_metadata,
            commands::link_series_to_catalog,
            commands::import_library_to_active_site,
            commands::get_catalog_info_for_series,
            commands::discover_catalog_card,
            commands::decide_catalog_card,
            commands::link_catalog_series,
            commands::list_swipe_history,
            commands::undo_swipe_entry,
            commands::get_deck_bans,
            commands::set_deck_bans,
            commands::backup_status,
            commands::set_google_credentials,
            commands::connect_drive,
            commands::disconnect_drive,
            commands::backup_now,
            commands::restore_latest,
            commands::auto_backup_if_due,
            commands::uninstall_app,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
