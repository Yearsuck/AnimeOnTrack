use crate::adapter::{animeytx::AnimeytxAdapter, SiteAdapter};
use crate::db::Db;
use crate::diff::new_episodes;
use crate::models::{Episode, Series};
use crate::player::{BrowserPlayer, EpisodePlayer};
use crate::scraper_engine::fetch_html;
use std::sync::Mutex;
use tauri::{AppHandle, State};

pub struct AppState {
    pub db: Mutex<Db>,
    pub source_id: Mutex<Option<i64>>,
}

const SOURCE_NAME: &str = "AnimeYT";

fn adapter() -> AnimeytxAdapter {
    AnimeytxAdapter
}

fn get_source_id(state: &State<AppState>) -> Result<i64, String> {
    state
        .source_id
        .lock()
        .unwrap()
        .ok_or_else(|| "no source configured; run scan_airing first".to_string())
}

/// First-run + manual re-scan: store base_url, scrape airing list, upsert series.
#[tauri::command]
pub async fn scan_airing(
    app: AppHandle,
    state: State<'_, AppState>,
    base_url: String,
) -> Result<Vec<Series>, String> {
    let a = adapter();
    let url = a.airing_url(&base_url);
    let html = fetch_html(&app, &url).await.map_err(|e| e.to_string())?;
    let series = a.parse_airing(&html).map_err(|e| e.to_string())?;
    if series.is_empty() {
        return Err("no series parsed; site layout may have changed".into());
    }
    let db = state.db.lock().unwrap();
    let src = db
        .upsert_source(SOURCE_NAME, base_url.trim_end_matches('/'))
        .map_err(|e| e.to_string())?;
    for s in &series {
        db.upsert_series(src, s).map_err(|e| e.to_string())?;
    }
    *state.source_id.lock().unwrap() = Some(src);
    db.list_airing(src).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn list_airing(state: State<'_, AppState>) -> Result<Vec<Series>, String> {
    let src = get_source_id(&state)?;
    let db = state.db.lock().unwrap();
    db.list_airing(src).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn set_followed(
    state: State<'_, AppState>,
    series_id: i64,
    followed: bool,
) -> Result<(), String> {
    let db = state.db.lock().unwrap();
    db.set_followed(series_id, followed).map_err(|e| e.to_string())
}

/// For each followed series: scrape its page, insert new episodes. Returns count of new episodes.
#[tauri::command]
pub async fn refresh(app: AppHandle, state: State<'_, AppState>) -> Result<i64, String> {
    let src = get_source_id(&state)?;
    let followed = {
        let db = state.db.lock().unwrap();
        db.list_followed(src).map_err(|e| e.to_string())?
    };
    let a = adapter();
    let mut total_new = 0i64;
    for s in followed {
        let html = match fetch_html(&app, &s.url).await {
            Ok(h) => h,
            Err(_) => continue, // unreachable series: skip, keep cached data
        };
        let scraped = match a.parse_series(&html) {
            Ok(eps) => eps,
            Err(_) => continue, // layout change: skip, don't wipe
        };
        {
            let db = state.db.lock().unwrap();
            let known = db.existing_episode_urls(s.id).map_err(|e| e.to_string())?;
            for mut e in new_episodes(&scraped, &known) {
                e.series_id = s.id;
                db.insert_episode(&e).map_err(|e| e.to_string())?;
                total_new += 1;
            }
        }
        // polite delay between series
        tokio::time::sleep(std::time::Duration::from_millis(800)).await;
    }
    Ok(total_new)
}

#[tauri::command]
pub fn list_pending(state: State<'_, AppState>) -> Result<Vec<PendingItem>, String> {
    let db = state.db.lock().unwrap();
    let rows = db.list_pending().map_err(|e| e.to_string())?;
    Ok(rows
        .into_iter()
        .map(|(s, e)| PendingItem { series: s, episode: e })
        .collect())
}

#[derive(serde::Serialize)]
pub struct PendingItem {
    pub series: Series,
    pub episode: Episode,
}

#[tauri::command]
pub fn pending_count(state: State<'_, AppState>) -> Result<i64, String> {
    let db = state.db.lock().unwrap();
    db.pending_count().map_err(|e| e.to_string())
}

/// Open an episode in the browser. Does NOT mark it seen — the user marks
/// seen/unseen explicitly via `set_seen`.
#[tauri::command]
pub fn open_episode(app: AppHandle, url: String) -> Result<(), String> {
    let ep = Episode {
        id: 0,
        series_id: 0,
        number: String::new(),
        title: None,
        url,
        released_at: None,
        seen: false,
    };
    BrowserPlayer.open(&app, &ep).map_err(|e| e.to_string())
}

/// Mark an episode seen or unseen (persisted).
#[tauri::command]
pub fn set_seen(state: State<'_, AppState>, episode_id: i64, seen: bool) -> Result<(), String> {
    let db = state.db.lock().unwrap();
    db.set_seen(episode_id, seen).map_err(|e| e.to_string())
}

/// All episodes of a series (progress view), oldest first.
#[tauri::command]
pub fn list_episodes(state: State<'_, AppState>, series_id: i64) -> Result<Vec<Episode>, String> {
    let db = state.db.lock().unwrap();
    db.list_series_episodes(series_id).map_err(|e| e.to_string())
}
