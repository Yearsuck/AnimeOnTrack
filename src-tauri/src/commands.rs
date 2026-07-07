use crate::adapter::{animeytx::AnimeytxAdapter, SiteAdapter};
use crate::db::Db;
use crate::diff::new_episodes;
use crate::models::{Episode, Series};
use crate::player::{BrowserPlayer, EpisodePlayer};
use crate::scraper_engine::{fetch_cover_image, fetch_html, ScrapeResult};
use serde::Serialize;
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, State};

#[derive(Serialize, Clone)]
struct RefreshProgress {
    current: usize,
    total: usize,
    title: String,
}

fn emit_refresh_progress(app: &AppHandle, current: usize, total: usize, title: &str) {
    let _ = app.emit(
        "refresh-progress",
        RefreshProgress { current, total, title: title.to_string() },
    );
}

pub struct AppState {
    pub db: Mutex<Db>,
    pub source_id: Mutex<Option<i64>>,
}

const SOURCE_NAME: &str = "AnimeYT";
const MIRRORS_KEY: &str = "mirror_urls";

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

fn normalize(url: &str) -> String {
    url.trim().trim_end_matches('/').to_string()
}

fn load_mirrors(db: &Db) -> Result<Vec<String>, String> {
    let raw = db.get_setting(MIRRORS_KEY).map_err(|e| e.to_string())?;
    Ok(raw
        .map(|s| s.lines().map(normalize).filter(|l| !l.is_empty()).collect())
        .unwrap_or_default())
}

fn save_mirrors(db: &Db, mirrors: &[String]) -> Result<(), String> {
    db.set_setting(MIRRORS_KEY, &mirrors.join("\n"))
        .map_err(|e| e.to_string())
}

/// Add `url` to the front of `mirrors` if not already present (case-insensitive),
/// otherwise leave the existing order alone.
fn with_mirror(mirrors: Vec<String>, url: &str) -> Vec<String> {
    let url = normalize(url);
    if mirrors.iter().any(|m| m.eq_ignore_ascii_case(&url)) {
        mirrors
    } else {
        let mut out = vec![url];
        out.extend(mirrors);
        out
    }
}

/// Try `path` (e.g. "/anime-en-emision/" or "/tv/some-series/") against each
/// mirror in order, returning the first scrape that ALSO parses to something
/// non-empty via `parse`, along with the mirror that worked.
///
/// A mirror can fail two different ways: the page doesn't load at all (network
/// error, Cloudflare doesn't clear), or the page loads fine but isn't actually
/// this site (e.g. a URL that turns out to be a different, incompatible anime
/// site rather than a same-layout clone) — our selectors then find nothing.
/// Both must fall through to the next mirror, or one bad entry anywhere in the
/// list can break every scan, even when a perfectly good mirror is right below
/// it.
async fn scrape_via_mirrors<T>(
    app: &AppHandle,
    mirrors: &[String],
    path: &str,
    parse: impl Fn(&str) -> Result<Vec<T>, anyhow::Error>,
) -> Result<(ScrapeResult, Vec<T>, String), String> {
    if mirrors.is_empty() {
        return Err("no hay ninguna web configurada".into());
    }
    let mut last_err = String::new();
    for mirror in mirrors {
        let url = format!("{mirror}{path}");
        match fetch_html(app, &url).await {
            Ok(scraped) => match parse(&scraped.html) {
                Ok(items) if !items.is_empty() => {
                    return Ok((scraped, items, mirror.clone()));
                }
                Ok(_) => {
                    last_err = format!(
                        "{mirror}: la página cargó pero no encajaba con este sitio (¿no es un mirror real?)"
                    )
                }
                Err(e) => last_err = format!("{mirror}: {e}"),
            },
            Err(e) => last_err = format!("{mirror}: {e}"),
        }
    }
    Err(format!("ninguna web funcionó; último error: {last_err}"))
}

async fn scan_airing_via_mirrors(
    app: &AppHandle,
    state: &State<'_, AppState>,
    mirrors: Vec<String>,
) -> Result<Vec<Series>, String> {
    let a = adapter();
    emit_refresh_progress(app, 0, 1, "Escaneando listado de estrenos");
    // airing_url() just appends a fixed path; reuse it against an empty base to get that path alone.
    let path = a.airing_url("").to_string();
    let (_scraped, series, working_mirror) =
        scrape_via_mirrors(app, &mirrors, &path, |html| a.parse_airing(html)).await?;
    emit_refresh_progress(app, 1, 1, "Listado completo");
    // Cover images are intentionally NOT fetched here: doing it for every
    // series on the airing list (~150 at once) reads as scraping abuse to
    // Cloudflare and gets rate-limited regardless of session validity. Covers
    // are fetched one at a time in `refresh`, only for followed series.

    let db = state.db.lock().unwrap();
    save_mirrors(&db, &mirrors)?;
    let src = db
        .upsert_source(SOURCE_NAME, &working_mirror)
        .map_err(|e| e.to_string())?;
    for s in &series {
        db.upsert_series(src, s).map_err(|e| e.to_string())?;
    }
    *state.source_id.lock().unwrap() = Some(src);
    db.list_airing(src).map_err(|e| e.to_string())
}

/// First-run scan: seed the mirror list with `base_url` (kept first if new),
/// then scan the airing list trying every configured mirror in order.
#[tauri::command]
pub async fn scan_airing(
    app: AppHandle,
    state: State<'_, AppState>,
    base_url: String,
) -> Result<Vec<Series>, String> {
    let existing = {
        let db = state.db.lock().unwrap();
        load_mirrors(&db)?
    };
    let mirrors = with_mirror(existing, &base_url);
    scan_airing_via_mirrors(&app, &state, mirrors).await
}

/// Re-scan the airing list using only the mirrors already configured in
/// Settings (no new URL supplied).
#[tauri::command]
pub async fn rescan_airing(app: AppHandle, state: State<'_, AppState>) -> Result<Vec<Series>, String> {
    let mirrors = {
        let db = state.db.lock().unwrap();
        load_mirrors(&db)?
    };
    scan_airing_via_mirrors(&app, &state, mirrors).await
}

#[tauri::command]
pub fn get_mirrors(state: State<'_, AppState>) -> Result<Vec<String>, String> {
    let db = state.db.lock().unwrap();
    load_mirrors(&db)
}

/// Save the mirror list. If it would end up without the site the app is
/// currently actually using (`sources.base_url`), that site is kept at the
/// front regardless — otherwise a Settings edit can silently strand every
/// future scan with no working entry at all.
#[tauri::command]
pub fn set_mirrors(state: State<'_, AppState>, urls: Vec<String>) -> Result<(), String> {
    let db = state.db.lock().unwrap();
    let mut cleaned: Vec<String> = urls.iter().map(|u| normalize(u)).filter(|u| !u.is_empty()).collect();
    if let Ok(Some(src_id)) = state.source_id.lock().map(|g| *g) {
        if let Ok(Some(base_url)) = db.get_source_base_url(src_id) {
            let base_url = normalize(&base_url);
            if !cleaned.iter().any(|m| m.eq_ignore_ascii_case(&base_url)) {
                cleaned.insert(0, base_url);
            }
        }
    }
    save_mirrors(&db, &cleaned)
}

#[tauri::command]
pub fn list_airing(state: State<'_, AppState>) -> Result<Vec<Series>, String> {
    let src = get_source_id(&state)?;
    let db = state.db.lock().unwrap();
    db.list_airing(src).map_err(|e| e.to_string())
}

/// Followed series with episode counts, for the library view.
#[tauri::command]
pub fn list_library(state: State<'_, AppState>) -> Result<Vec<crate::models::LibraryItem>, String> {
    let src = get_source_id(&state)?;
    let db = state.db.lock().unwrap();
    db.list_library(src).map_err(|e| e.to_string())
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

/// For each followed series: scrape its page (falling back across mirrors),
/// insert new episodes. Returns count of new episodes.
#[tauri::command]
pub async fn refresh(app: AppHandle, state: State<'_, AppState>) -> Result<i64, String> {
    let src = get_source_id(&state)?;
    let (followed, mirrors) = {
        let db = state.db.lock().unwrap();
        (
            db.list_followed(src).map_err(|e| e.to_string())?,
            load_mirrors(&db)?,
        )
    };
    let a = adapter();
    let mut total_new = 0i64;
    let total_series = followed.len();
    for (idx, s) in followed.into_iter().enumerate() {
        emit_refresh_progress(&app, idx, total_series, &s.title);
        let path = match url::Url::parse(&s.url) {
            Ok(u) => format!("{}{}", u.path(), u.query().map(|q| format!("?{q}")).unwrap_or_default()),
            Err(_) => continue, // malformed stored url: skip, keep cached data
        };
        let (_scraped, eps, working_mirror) =
            match scrape_via_mirrors(&app, &mirrors, &path, |html| a.parse_series(html)).await {
                Ok(r) => r,
                Err(_) => continue, // unreachable/incompatible on every mirror: skip, keep cached data
            };
        {
            let db = state.db.lock().unwrap();
            let new_url = format!("{working_mirror}{path}");
            if new_url != s.url {
                db.update_series_url(s.id, &new_url).map_err(|e| e.to_string())?;
            }
            let known = db.existing_episode_urls(s.id).map_err(|e| e.to_string())?;
            for mut e in new_episodes(&eps, &known) {
                e.series_id = s.id;
                db.insert_episode(&e).map_err(|e| e.to_string())?;
                total_new += 1;
            }
        }

        // One cover fetch per followed series per refresh — never in bulk.
        // Skip once it's already a fetched data: URI so we don't re-fetch on
        // every refresh; a failure here just leaves the remote (broken) url
        // in place to retry next time, it never blocks episode updates above.
        if let Some(remote) = &s.cover_url {
            if !remote.starts_with("data:") {
                if let Ok(data_uri) = fetch_cover_image(&app, remote).await {
                    let db = state.db.lock().unwrap();
                    let _ = db.update_series_cover(s.id, &data_uri);
                }
            }
        }

        // polite delay between series
        tokio::time::sleep(std::time::Duration::from_millis(800)).await;
    }
    emit_refresh_progress(&app, total_series, total_series, "Completado");
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
