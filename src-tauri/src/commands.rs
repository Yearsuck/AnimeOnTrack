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
use crate::scraper_engine::{fetch_cover_image, fetch_html, ScrapeResult};
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

/// Prefix for the per-site cached genre-archive list
/// (`genre_list:{site_id}`) — see `ensure_genre_list`.
const GENRE_LIST_KEY_PREFIX: &str = "genre_list";

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
/// `use_cache: true` consults the app-wide short-TTL HTML cache before
/// opening a scraper window (and every successful fetch populates it either
/// way). Callers whose purpose is *detecting change* — refresh()'s listing
/// scan and per-series fetches, the user-triggered airing rescan — must pass
/// `false`: serving them minutes-old HTML would silently defeat the check
/// they exist to perform. A cached page that no longer parses non-empty
/// falls through to a real fetch rather than failing the mirror.
async fn scrape_via_mirrors<T>(
    app: &AppHandle,
    mirrors: &[String],
    path: &str,
    use_cache: bool,
    parse: impl Fn(&str) -> Result<Vec<T>, anyhow::Error>,
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
                if let Ok(items) = parse(&html) {
                    if !items.is_empty() {
                        return Ok((ScrapeResult { html }, items, mirror.clone()));
                    }
                }
                // Cached HTML didn't satisfy this parse — fall through to a
                // real fetch below instead of treating the mirror as broken.
            }
        }
        match fetch_html(app, &url).await {
            Ok(scraped) => match parse(&scraped.html) {
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








/// Pick a taste-weighted genre (mirroring `discover_swipe_card`'s scheme:
/// `get_genre_affinity` + `weighted_pick_index`, uniform fallback when
/// nothing's been decided yet — but see the dampening note below) and ask
/// the DB for a taste-scored undecided, quality-floored catalog entry in it
/// (`recommend::pick_recommended`, see
/// docs/superpowers/specs/2026-07-12-discover-recommendation-engine-design.md).
/// Local + instant (no live AniList call per swipe, so no rate-limit
/// exposure from normal browsing). Catalog cards carry no episode data
/// (AniList is metadata-only), so they're decided through
/// `decide_catalog_card` rather than `decide_swipe`, which assumes a
/// scraped-site URL it can fetch an episode list from.
///
/// The outer genre-pick weights are run through
/// `recommend::dampen_genre_weight` (sub-linear, `w' = max(0,score)^0.6`)
/// before `weighted_pick_index` — raw affinity sums let one heavily-followed
/// genre swamp every other candidate; dampening compresses that lead without
/// flipping the order, so the deck still favors the user's top genre without
/// collapsing into showing only that genre. Cold start (nothing
/// followed/decided) still degrades to `weighted_pick_index`'s uniform
/// fallback: dampening never turns a non-positive score into a positive one.
///
/// `Ok(None)` means the deck is genuinely exhausted: every candidate genre
/// (after excluding Hentai/Ecchi) either has zero synced titles passing the
/// quality floor or every one of them has already been decided, and
/// `MAX_GENRE_ATTEMPTS` genre picks in a row all came up empty.
/// `recommended` selects the deck mode (see `DiscoverModeToggle` on the
/// frontend, persisted in localStorage `aot.discoverMode`): `true` is the
/// taste-weighted behavior documented above, unchanged. `false` ("Aleatorio")
/// builds **empty** affinity maps instead — which makes the outer genre pick
/// degrade to `weighted_pick_index`'s uniform fallback (all-dampened weights
/// are 0, same cold-start path) — and threads `recommended` into
/// `random_catalog_anime_in_genre` so the inner per-candidate pick bypasses
/// scoring too. Empty maps ALONE are not enough for the inner pick: with
/// `recommended=true` `score_candidate`'s quality term stays active even with
/// empty genre/format maps, still biasing toward high `average_score` — see
/// docs/superpowers/specs/2026-07-13-discover-recommendation-toggle-design.md.




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
    let (refresh, file_id) = {
        let db = state.db.lock().unwrap();
        let refresh = db
            .get_setting("gdrive_refresh_token")
            .ok()
            .flatten()
            .ok_or("Not connected to Google Drive")?;
        let file_id = db
            .get_setting("gdrive_file_id")
            .ok()
            .flatten()
            .ok_or("No backup found in Drive yet")?;
        (refresh, file_id)
    };
    let token = backup_lib::access_token(&refresh).await?;
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
