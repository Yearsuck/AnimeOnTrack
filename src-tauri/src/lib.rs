mod adapter;
mod anilist;
mod commands;
mod db;
mod diff;
mod genres;
mod html_cache;
mod matching;
mod models;
mod player;
mod scraper_engine;
mod swipe;

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
                last_swiped_series_id: Mutex::new(None),
                html_cache: Mutex::new(html_cache::HtmlCache::default()),
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::scan_airing,
            commands::list_airing,
            commands::list_library,
            commands::get_genre_stats,
            commands::get_type_stats,
            commands::get_watch_summary,
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
            commands::discover_catalog_card,
            commands::decide_catalog_card,
            commands::link_catalog_series,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
