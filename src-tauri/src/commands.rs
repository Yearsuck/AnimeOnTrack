use crate::adapter::{self, SiteAdapter};
use crate::backup as backup_lib;
use crate::dates::parse_spanish_date;
use crate::db::Db;
use crate::diff::new_episodes;
use crate::models::{
    AiringItem, BackupStatus, Episode, FinishedCard, GenreAffinity, GenreStat, Series, SeriesDetail,
    SeriesGraphNode, TypeStat, WatchInsights, WatchSummary,
};
use crate::player::{AppWindowPlayer, EpisodePlayer};
use crate::scraper_engine::{fetch_cover_image, fetch_html_with_script, ScrapeResult};
use crate::swipe::{pick_index, shuffle, undecided_cards, weighted_pick_index};
use serde::Serialize;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, Manager, State};

mod backup;
pub use backup::*;

mod stats;
pub use stats::*;

mod library;
pub use library::*;

mod seen;
pub use seen::*;

mod catalog;
pub use catalog::*;

mod discover;
pub use discover::*;

mod follow;
pub use follow::*;

mod mirrors;
pub use mirrors::*;

mod scan;
pub use scan::*;

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
    /// Stable slug (`adapter::SiteInfo::id`) of the currently-active site —
    /// the single source of truth `adapter_for`/per-site mirror keys read
    /// from. Decoupled from `source_id` (the DB row, which may still be
    /// `None` the very first time a newly-selected site is scanned).
    /// Restored at startup from the `active_site_id` setting, defaulting to
    /// `"animeytx"` for installs that predate multi-site support.
    pub active_site_id: Mutex<String>,
    /// Short-TTL rendered-HTML cache (optimization C, see `html_cache.rs`)
    /// consulted by `scrape_via_mirrors` — makes re-scraping a page just
    /// fetched by another flow (refresh → discover/link/backfill) free
    /// within the TTL. Never used for refresh()'s own fetches, whose whole
    /// point is checking for *changes*.
    pub html_cache: Mutex<crate::html_cache::HtmlCache>,
    /// Cards from a (genre, page) fetch not yet shown this session — lets
    /// discover_swipe_card serve ~10 swipes off one HTTP fetch instead of one
    /// fetch per swipe.
    pub swipe_buffer: Mutex<HashMap<(String, u32), Vec<FinishedCard>>>,
    /// Highest page number seen so far for each genre slug this session.
    pub swipe_last_page: Mutex<HashMap<String, u32>>,
    /// Cards handed out by discover_swipe_card, keyed by url, so decide_swipe
    /// (which only receives a url) can look up the card data to persist.
    pub swipe_served: Mutex<HashMap<String, FinishedCard>>,
    /// series.ids written by `decide_swipe`/`decide_catalog_card`, most
    /// recent at the front, capped at `SWIPE_HISTORY_CAP`. `undo_last_swipe`
    /// (Ctrl+Z) pops the front; `list_swipe_history`/`undo_swipe_entry` let
    /// the UI reach further back than just the most recent. Session-only
    /// "fix my last few misclicks" safety net (see the design spec) — not
    /// persisted across app restarts, not an audit log.
    pub swipe_history: Mutex<VecDeque<i64>>,
    /// Set while a library import (manual or the auto-import fired after an
    /// airing scan) is running, so overlapping scans can't launch a second
    /// concurrent per-series scrape sweep of the same site — that would double
    /// the Cloudflare-facing request rate. A plain flag (not a queue): once one
    /// import is in flight, later triggers no-op, and the next scan after it
    /// finishes picks up whatever is still missing.
    pub library_import_running: std::sync::atomic::AtomicBool,
}

/// How many recent swipe decisions `swipe_history` remembers.
const SWIPE_HISTORY_CAP: usize = 5;

/// Move `sid` to the front of `history` (removing any earlier copy first, so
/// a re-decided id is never held twice — see
/// `push_history_dedups_a_repeated_id_instead_of_holding_it_twice`), evicting
/// the oldest entry past `SWIPE_HISTORY_CAP`. Factored out as a plain
/// function over `VecDeque` (no `State<AppState>`) so it's unit-testable
/// directly.
fn push_history(history: &mut VecDeque<i64>, sid: i64) {
    history.retain(|&id| id != sid);
    history.push_front(sid);
    history.truncate(SWIPE_HISTORY_CAP);
}

/// The active site's adapter, looked up from `state.active_site_id`. `Err`
/// only if `active_site_id` somehow holds a slug `adapter::adapter_for`
/// doesn't recognize (can't happen through normal use — `set_active_site`
/// only ever writes a slug from `all_sites()`).
fn get_active_adapter(state: &State<AppState>) -> Result<Box<dyn SiteAdapter>, String> {
    let site_id = state.active_site_id.lock().unwrap().clone();
    adapter::adapter_for(&site_id).ok_or_else(|| format!("sitio desconocido: {site_id}"))
}

fn get_active_site_id(state: &State<AppState>) -> String {
    state.active_site_id.lock().unwrap().clone()
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
///
/// Thin wrapper over `scrape_via_mirrors_with_script` for the common case
/// (no adapter episode script needed) — see that function for the full doc.
async fn scrape_via_mirrors<T>(
    app: &AppHandle,
    mirrors: &[String],
    path: &str,
    use_cache: bool,
    parse: impl Fn(&ScrapeResult) -> Result<Vec<T>, anyhow::Error>,
) -> Result<(ScrapeResult, Vec<T>, String), String> {
    scrape_via_mirrors_with_script(app, mirrors, path, use_cache, None, parse).await
}

/// Same as `scrape_via_mirrors`, plus an adapter's optional
/// `episode_fetch_script` (see `SiteAdapter`) threaded through to
/// `fetch_html_with_script` — the only two call sites that need episode
/// data (`fetch_episode_list_for`, `fetch_series_episodes`) pass one; every
/// other call site passes `None`, which is byte-identical to the pre-jkanime
/// fetch path.
///
/// `use_cache: true` consults the app-wide short-TTL HTML cache before
/// opening a scraper window (and every successful fetch populates it either
/// way). Callers whose purpose is *detecting change* — refresh()'s listing
/// scan and per-series fetches, the user-triggered airing rescan — must pass
/// `false`: serving them minutes-old HTML would silently defeat the check
/// they exist to perform. A cached page that no longer parses non-empty
/// falls through to a real fetch rather than failing the mirror. A cache hit
/// never carries `extra` (it was never re-run through the page) — for a site
/// with an episode script, that means `parse` sees `extra: None` and falls
/// back to parsing plain `html`, which never has the episode data, so the
/// parse comes back empty and this naturally falls through to a real fetch.
async fn scrape_via_mirrors_with_script<T>(
    app: &AppHandle,
    mirrors: &[String],
    path: &str,
    use_cache: bool,
    extra_script: Option<&str>,
    parse: impl Fn(&ScrapeResult) -> Result<Vec<T>, anyhow::Error>,
) -> Result<(ScrapeResult, Vec<T>, String), String> {
    if mirrors.is_empty() {
        return Err("no hay ninguna web configurada".into());
    }
    let mut last_err = String::new();
    for mirror in mirrors {
        let url = format!("{mirror}{path}");
        if use_cache {
            let cached = {
                let state = app.state::<AppState>();
                let mut cache = state.html_cache.lock().unwrap();
                cache.get(&url, std::time::Instant::now())
            };
            if let Some(html) = cached {
                let scraped = ScrapeResult { html, extra: None };
                if let Ok(items) = parse(&scraped) {
                    if !items.is_empty() {
                        return Ok((scraped, items, mirror.clone()));
                    }
                }
                // Cached HTML didn't satisfy this parse — fall through to a
                // real fetch below instead of treating the mirror as broken.
            }
        }
        match fetch_html_with_script(app, &url, extra_script).await {
            Ok(scraped) => match parse(&scraped) {
                Ok(items) if !items.is_empty() => {
                    let state = app.state::<AppState>();
                    let mut cache = state.html_cache.lock().unwrap();
                    cache.put(&url, scraped.html.clone(), std::time::Instant::now());
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


/// The currently-active site (`state.active_site_id`), for the Settings
/// selector's initial value.
#[tauri::command]
pub fn get_active_site(state: State<'_, AppState>) -> Result<SiteSummary, String> {
    let site_id = get_active_site_id(&state);
    adapter::all_sites()
        .iter()
        .find(|s| s.id == site_id)
        .map(SiteSummary::from)
        .ok_or_else(|| format!("sitio activo desconocido: {site_id}"))
}






// ---------------------------------------------------------------------------
// Cloud backup (Google Drive appDataFolder) — see src/backup/. reqwest here
// only ever talks to accounts.google.com/oauth2.googleapis.com/googleapis.com
// (a normal API), never the scraped site; backup/ never imports
// scraper_engine.
// ---------------------------------------------------------------------------

pub(crate) fn backup_dir(app: &AppHandle) -> Result<std::path::PathBuf, String> {
    app.path().app_data_dir().map_err(|e| format!("app_data_dir: {e}"))
}

#[tauri::command]
pub async fn restore_latest(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    let dir = backup_dir(&app)?;
    let (client, refresh, file_id) = {
        let db = state.db.lock().unwrap();
        let client = backup_lib::configured_client(&db)
            .ok_or("Google credentials not configured")?;
        let refresh = db
            .get_setting("gdrive_refresh_token")
            .ok()
            .flatten()
            .ok_or("Not connected to Google Drive")?;
        let file_id = db.get_setting("gdrive_file_id").ok().flatten();
        (client, refresh, file_id)
    };
    let token = backup_lib::access_token(&client, &refresh).await?;
    // Falling back to a lookup by name is what makes "restore onto a new
    // machine" — the whole point of the feature — actually work: a fresh
    // install has an empty settings table, so `gdrive_file_id` is only ever
    // present on the machine that uploaded the backup in the first place.
    let file_id = match file_id {
        Some(id) => id,
        None => backup_lib::drive::find_backup_file(&token)
            .await?
            .ok_or("No backup found in Drive yet")?,
    };
    let bytes = backup_lib::drive::download_backup(&token, &file_id).await?;
    backup_lib::stage_restore(&bytes, &dir)?; // validates before staging
    app.restart();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_history_keeps_most_recent_at_front_and_caps_at_five() {
        let mut h = VecDeque::new();
        for sid in 1..=7 {
            push_history(&mut h, sid);
        }
        // Newest first, oldest two (1 and 2) evicted past the cap of 5.
        let got: Vec<i64> = h.iter().copied().collect();
        assert_eq!(got, vec![7, 6, 5, 4, 3]);
        assert_eq!(h.len(), SWIPE_HISTORY_CAP);
    }

    #[test]
    fn push_history_dedups_a_repeated_id_instead_of_holding_it_twice() {
        // Re-deciding the same series id (a card the reappearance race let a
        // concurrent picker re-serve, then the user swiped again) must move
        // it to the front WITHOUT leaving a second copy behind — a duplicate
        // sid is what made the "Últimas clasificadas" strip render the same
        // title twice (list_swipe_history has no dedup of its own).
        let mut h = VecDeque::new();
        push_history(&mut h, 10);
        push_history(&mut h, 20);
        push_history(&mut h, 10);
        assert_eq!(h.iter().copied().collect::<Vec<i64>>(), vec![10, 20]);
        assert_eq!(h.len(), 2, "10 must appear exactly once, not twice");
    }

}
