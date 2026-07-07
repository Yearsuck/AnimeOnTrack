mod adapter;
mod commands;
mod db;
mod diff;
mod models;
mod player;
mod scraper_engine;

use commands::AppState;
use std::sync::Mutex;
use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let dir = app.path().app_data_dir().expect("app data dir");
            std::fs::create_dir_all(&dir).ok();
            let db_path = dir.join("animeontrack.sqlite");
            let db = db::Db::open(db_path.to_str().unwrap()).expect("open db");
            // Restore last source id if a single source exists.
            let source_id: Option<i64> = db
                .conn
                .query_row("SELECT id FROM sources LIMIT 1", [], |r| r.get(0))
                .ok();
            app.manage(AppState {
                db: Mutex::new(db),
                source_id: Mutex::new(source_id),
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::scan_airing,
            commands::list_airing,
            commands::list_library,
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
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
