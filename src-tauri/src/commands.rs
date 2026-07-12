use crate::adapter::{self, SiteAdapter};
use crate::db::Db;
use crate::diff::new_episodes;
use crate::models::{
    Episode, FinishedCard, GenreAffinity, GenreStat, Series, SeriesDetail, SeriesGraphNode, TypeStat,
    WatchSummary,
};
use crate::player::{AppWindowPlayer, EpisodePlayer};
use crate::scraper_engine::{fetch_cover_image, fetch_html, ScrapeResult};
use crate::swipe::{pick_index, shuffle, undecided_cards, weighted_pick_index};
use serde::Serialize;
use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, Manager, State};

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

/// Push `sid` to the front of `history`, evicting the oldest entry past
/// `SWIPE_HISTORY_CAP`. Factored out as a plain function over `VecDeque`
/// (no `State<AppState>`) so it's unit-testable directly.
fn push_history(history: &mut VecDeque<i64>, sid: i64) {
    history.push_front(sid);
    history.truncate(SWIPE_HISTORY_CAP);
}

/// Record a fresh decision at the front of the session's undo/history
/// deque. Shared by `decide_swipe` and `decide_catalog_card` so both
/// sources feed the same history the frontend's strip and Ctrl+Z read from.
fn push_swipe_history(state: &State<'_, AppState>, sid: i64) {
    push_history(&mut state.swipe_history.lock().unwrap(), sid);
}

/// Prefix for the per-site mirror-list settings key (`mirror_urls:{site_id}`).
/// The old global `mirror_urls` key (no site suffix) is left in the DB
/// untouched as a rollback path — see `load_mirrors`'s one-time migration.
const MIRRORS_KEY_PREFIX: &str = "mirror_urls";
/// Legacy pre-multi-site global mirrors key. Migrated once into
/// `mirror_urls:animeytx` by `load_mirrors` the first time that site-scoped
/// key is read and found empty.
const LEGACY_MIRRORS_KEY: &str = "mirror_urls";
/// Prefix for the per-site cached genre-archive list
/// (`genre_list:{site_id}`) — see `ensure_genre_list`.
const GENRE_LIST_KEY_PREFIX: &str = "genre_list";
/// Settings key holding the stable slug of the currently-active site
/// (`adapter::SiteInfo::id`). Absent on installs that predate multi-site
/// support — those default to `"animeytx"`, the only site that ever existed
/// before this key did.
const ACTIVE_SITE_KEY: &str = "active_site_id";
const DEFAULT_SITE_ID: &str = "animeytx";

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

/// Load the mirror list for `site_id`. One-time migration: if the site-scoped
/// key has never been written and this is the original site (`"animeytx"`),
/// fall back to the pre-multi-site global `mirror_urls` key, and persist it
/// forward under the site-scoped key so future reads don't need this check —
/// the old global key is left in place either way (harmless, and a rollback
/// path), per the design spec.
fn load_mirrors(db: &Db, site_id: &str) -> Result<Vec<String>, String> {
    let key = format!("{MIRRORS_KEY_PREFIX}:{site_id}");
    let raw = db.get_setting(&key).map_err(|e| e.to_string())?;
    if let Some(raw) = raw {
        return Ok(parse_mirrors(&raw));
    }
    if site_id == DEFAULT_SITE_ID {
        if let Some(legacy) = db.get_setting(LEGACY_MIRRORS_KEY).map_err(|e| e.to_string())? {
            let mirrors = parse_mirrors(&legacy);
            db.set_setting(&key, &legacy).map_err(|e| e.to_string())?;
            return Ok(mirrors);
        }
    }
    Ok(Vec::new())
}

fn parse_mirrors(raw: &str) -> Vec<String> {
    raw.lines().map(normalize).filter(|l| !l.is_empty()).collect()
}

fn save_mirrors(db: &Db, site_id: &str, mirrors: &[String]) -> Result<(), String> {
    let key = format!("{MIRRORS_KEY_PREFIX}:{site_id}");
    db.set_setting(&key, &mirrors.join("\n")).map_err(|e| e.to_string())
}

fn load_genre_list(db: &Db, site_id: &str) -> Result<Vec<(String, String)>, String> {
    let key = format!("{GENRE_LIST_KEY_PREFIX}:{site_id}");
    let raw = db.get_setting(&key).map_err(|e| e.to_string())?;
    match raw {
        Some(s) => serde_json::from_str(&s).map_err(|e| e.to_string()),
        None => Ok(Vec::new()),
    }
}

fn save_genre_list(db: &Db, site_id: &str, list: &[(String, String)]) -> Result<(), String> {
    let key = format!("{GENRE_LIST_KEY_PREFIX}:{site_id}");
    let raw = serde_json::to_string(list).map_err(|e| e.to_string())?;
    db.set_setting(&key, &raw).map_err(|e| e.to_string())
}

/// Cached genre (slug, name) list, scraped once per install (per site) and
/// reused after (mirrors the `mirror_urls` settings-cache pattern already
/// used elsewhere). A site with no genre archive (`genre_list_url` returning
/// `""`, the trait default) always scrapes an empty list, caches it, and
/// `discover_swipe_card`'s caller sees `Ok(vec![])` — same "no genres" error
/// path as an actual scrape failure would hit.
async fn ensure_genre_list(
    app: &AppHandle,
    db_mirrors: &[String],
    state: &State<'_, AppState>,
    a: &dyn SiteAdapter,
    site_id: &str,
) -> Result<Vec<(String, String)>, String> {
    let cached = {
        let db = state.db.lock().unwrap();
        load_genre_list(&db, site_id)?
    };
    if !cached.is_empty() {
        return Ok(cached);
    }
    let (_scraped, pairs, _mirror) =
        scrape_via_mirrors(app, db_mirrors, &a.genre_list_url(""), true, |html| a.parse_genre_list(html)).await?;
    let db = state.db.lock().unwrap();
    save_genre_list(&db, site_id, &pairs)?;
    Ok(pairs)
}

fn path_of(url: &str) -> Result<String, String> {
    let u = url::Url::parse(url).map_err(|_| format!("url inválida: {url}"))?;
    Ok(format!("{}{}", u.path(), u.query().map(|q| format!("?{q}")).unwrap_or_default()))
}

/// Fetch and parse a series detail page, falling through mirrors the same
/// way every other scrape does. An empty genre list is treated the same as
/// "page loaded but didn't parse" (see `scrape_via_mirrors`'s doc comment) —
/// it means this mirror's detail-page markup didn't match, not that the
/// series genuinely has zero genres.
async fn fetch_series_detail(
    app: &AppHandle,
    mirrors: &[String],
    series_url: &str,
    a: &dyn SiteAdapter,
) -> Result<SeriesDetail, String> {
    let path = path_of(series_url)?;
    let (_scraped, details, _mirror) = scrape_via_mirrors(app, mirrors, &path, true, |html| {
        let d = a.parse_series_detail(html)?;
        if d.genres.is_empty() {
            Err(anyhow::anyhow!("no genres parsed (likely wrong/incompatible mirror)"))
        } else {
            Ok(vec![d])
        }
    })
    .await?;
    details.into_iter().next().ok_or_else(|| "empty series detail page".to_string())
}

/// Backfill one series' genres/kind if it has no `series_genres` rows yet.
/// Returns true if a fetch was attempted (regardless of success) — used by
/// the caller to decide whether to apply the polite inter-series delay.
/// Locks `db` only for the short sync checks/writes, never across the
/// network `.await`, same discipline `refresh()`'s loop already follows.
async fn backfill_series_genre_if_missing(
    app: &AppHandle,
    db: &Mutex<Db>,
    mirrors: &[String],
    series: &Series,
    a: &dyn SiteAdapter,
) -> bool {
    let needs = {
        let db = db.lock().unwrap();
        db.series_needs_genre_backfill(series.id).unwrap_or(false)
    };
    if !needs {
        return false;
    }
    if let Ok(detail) = fetch_series_detail(app, mirrors, &series.url, a).await {
        let db = db.lock().unwrap();
        let _ = db.insert_series_genres(series.id, &detail.genres);
        if let Some(kind) = &detail.kind {
            let _ = db.set_kind(series.id, kind);
        }
    }
    true
}

async fn fetch_episode_list_for(
    app: &AppHandle,
    mirrors: &[String],
    series_url: &str,
    a: &dyn SiteAdapter,
) -> Result<Vec<Episode>, String> {
    let path = path_of(series_url)?;
    let (_scraped, eps, _mirror) =
        scrape_via_mirrors(app, mirrors, &path, true, |html| a.parse_series(html)).await?;
    Ok(eps)
}

fn slug_from_url(url: &str) -> String {
    url.trim_end_matches('/').rsplit('/').next().unwrap_or("").to_string()
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

async fn scan_airing_via_mirrors(
    app: &AppHandle,
    state: &State<'_, AppState>,
    mirrors: Vec<String>,
    a: &dyn SiteAdapter,
    site_id: &str,
) -> Result<Vec<Series>, String> {
    emit_refresh_progress(app, 0, 1, "Escaneando listado de estrenos");
    // airing_url() just appends a fixed path; reuse it against an empty base to get that path alone.
    let path = a.airing_url("").to_string();
    let (_scraped, series, working_mirror) =
        scrape_via_mirrors(app, &mirrors, &path, false, |html| a.parse_airing(html)).await?;
    emit_refresh_progress(app, 1, 1, "Listado completo");
    // Cover images are intentionally NOT fetched here: doing it for every
    // series on the airing list (~150 at once) reads as scraping abuse to
    // Cloudflare and gets rate-limited regardless of session validity. Covers
    // are fetched one at a time in `refresh`, only for followed series.

    let site_name = adapter::all_sites()
        .iter()
        .find(|s| s.id == site_id)
        .map(|s| s.name)
        .unwrap_or(site_id);

    let db = state.db.lock().unwrap();
    save_mirrors(&db, site_id, &mirrors)?;
    let src = db
        .upsert_source(site_name, &working_mirror, site_id)
        .map_err(|e| e.to_string())?;
    for s in &series {
        db.upsert_series(src, s).map_err(|e| e.to_string())?;
    }
    *state.source_id.lock().unwrap() = Some(src);
    db.list_airing(src).map_err(|e| e.to_string())
}

/// First-run scan: seed the mirror list with `base_url` (kept first if new),
/// then scan the airing list trying every configured mirror in order. Always
/// scans the currently-active site (`state.active_site_id`) — after a
/// Settings site switch, that's already been updated before this is called.
#[tauri::command]
pub async fn scan_airing(
    app: AppHandle,
    state: State<'_, AppState>,
    base_url: String,
) -> Result<Vec<Series>, String> {
    let site_id = get_active_site_id(&state);
    let a = get_active_adapter(&state)?;
    let existing = {
        let db = state.db.lock().unwrap();
        load_mirrors(&db, &site_id)?
    };
    let mirrors = with_mirror(existing, &base_url);
    scan_airing_via_mirrors(&app, &state, mirrors, a.as_ref(), &site_id).await
}

/// Re-scan the airing list using only the mirrors already configured in
/// Settings for the active site (no new URL supplied).
#[tauri::command]
pub async fn rescan_airing(app: AppHandle, state: State<'_, AppState>) -> Result<Vec<Series>, String> {
    let site_id = get_active_site_id(&state);
    let a = get_active_adapter(&state)?;
    let mirrors = {
        let db = state.db.lock().unwrap();
        load_mirrors(&db, &site_id)?
    };
    scan_airing_via_mirrors(&app, &state, mirrors, a.as_ref(), &site_id).await
}

#[tauri::command]
pub fn get_mirrors(state: State<'_, AppState>) -> Result<Vec<String>, String> {
    let site_id = get_active_site_id(&state);
    let db = state.db.lock().unwrap();
    load_mirrors(&db, &site_id)
}

/// Save the mirror list for the active site. If it would end up without the
/// site the app is currently actually using (`sources.base_url`), that URL is
/// kept at the front regardless — otherwise a Settings edit can silently
/// strand every future scan with no working entry at all.
#[tauri::command]
pub fn set_mirrors(state: State<'_, AppState>, urls: Vec<String>) -> Result<(), String> {
    let site_id = get_active_site_id(&state);
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
    save_mirrors(&db, &site_id, &cleaned)
}

/// Plain-data view of `adapter::SiteInfo` for the frontend — `SiteInfo`
/// itself holds `&'static str`s, not serializable across the Tauri IPC
/// boundary as-is.
#[derive(Serialize, Clone, Debug)]
pub struct SiteSummary {
    pub id: String,
    pub name: String,
    pub default_base_url: String,
}

impl From<&adapter::SiteInfo> for SiteSummary {
    fn from(s: &adapter::SiteInfo) -> Self {
        SiteSummary { id: s.id.to_string(), name: s.name.to_string(), default_base_url: s.default_base_url.to_string() }
    }
}

/// Every site the Settings selector can offer, in `adapter::all_sites()`
/// order.
#[tauri::command]
pub fn list_sites() -> Vec<SiteSummary> {
    adapter::all_sites().iter().map(SiteSummary::from).collect()
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

#[derive(Serialize, Debug)]
pub struct SiteSwitchResult {
    pub site: SiteSummary,
    /// `true` when this site has never been scanned before in this install
    /// (no `sources` row tagged with it yet) — the caller still needs to
    /// call `scan_airing` either way, but this lets the UI say "first scan
    /// of {site}…" vs "back to {site}" if it wants to.
    pub is_first_time: bool,
}

/// Pure-enough-to-test core of `set_active_site`: everything that reads/
/// writes the DB, returning the resulting `source_id` for the caller to
/// stash in `state.source_id`. Split out from the `#[tauri::command]` wrapper
/// so it's testable against a plain in-memory `Db` without a live `State`.
fn switch_site_core(db: &Db, site_id: &str) -> Result<(SiteSwitchResult, Option<i64>), String> {
    let info = adapter::all_sites()
        .iter()
        .find(|s| s.id == site_id)
        .ok_or_else(|| format!("sitio desconocido: {site_id}"))?;
    // Listed in all_sites() but no adapter implementation yet would silently
    // brick every scan after switching — refuse up front instead.
    if adapter::adapter_for(site_id).is_none() {
        return Err(format!("el sitio \"{}\" todavía no tiene un adaptador implementado", info.name));
    }

    let existing_mirrors = load_mirrors(db, site_id)?;
    if existing_mirrors.is_empty() {
        save_mirrors(db, site_id, &[info.default_base_url.to_string()])?;
    }
    let existing_source = db.get_source_id_for_site(site_id).map_err(|e| e.to_string())?;
    let is_first_time = existing_source.is_none();
    db.set_setting(ACTIVE_SITE_KEY, site_id).map_err(|e| e.to_string())?;

    Ok((SiteSwitchResult { site: SiteSummary::from(info), is_first_time }, existing_source))
}

/// Switch the active site: seed its mirror list (default_base_url, if it has
/// no mirrors configured yet) and its `sources` row (if it's never been
/// scanned before), then make it the one every other command routes through.
/// Does NOT itself scan the airing listing — the caller (Settings.tsx, after
/// its confirmation step) follows up with `scan_airing(result.site
/// .default_base_url)`, same first-run seeding path `scan_airing` already
/// has, so switching to a brand-new site and a fresh install behave
/// identically. Nothing about a site's existing `series`/`episodes` rows is
/// touched or deleted by switching away from it — see the design spec's
/// "library is per-site" scope note.
#[tauri::command]
pub fn set_active_site(state: State<'_, AppState>, site_id: String) -> Result<SiteSwitchResult, String> {
    let db = state.db.lock().unwrap();
    let (result, existing_source) = switch_site_core(&db, &site_id)?;
    *state.source_id.lock().unwrap() = existing_source;
    *state.active_site_id.lock().unwrap() = site_id;
    Ok(result)
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
pub fn get_genre_stats(state: State<'_, AppState>) -> Result<Vec<GenreStat>, String> {
    let src = get_source_id(&state)?;
    let db = state.db.lock().unwrap();
    db.get_genre_stats(src).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_type_stats(state: State<'_, AppState>) -> Result<Vec<TypeStat>, String> {
    let src = get_source_id(&state)?;
    let db = state.db.lock().unwrap();
    db.get_type_stats(src).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_watch_summary(state: State<'_, AppState>) -> Result<WatchSummary, String> {
    let src = get_source_id(&state)?;
    let db = state.db.lock().unwrap();
    db.get_watch_summary(src).map_err(|e| e.to_string())
}

/// Followed series with genres/kind/cover, for the 3D relationship graph.
/// The frontend builds the root/hub/link structure from this flat list.
#[tauri::command]
pub fn get_stats_graph(state: State<'_, AppState>) -> Result<Vec<SeriesGraphNode>, String> {
    let src = get_source_id(&state)?;
    let db = state.db.lock().unwrap();
    db.get_stats_graph_data(src).map_err(|e| e.to_string())
}

/// Backfill genres/kind for every followed series still missing
/// `series_genres` rows (not just the one-per-refresh-cycle `refresh()`
/// does). Same politeness delay between series as `refresh()`. Returns the
/// count of series that actually gained genres.
#[tauri::command]
pub async fn backfill_genres(app: AppHandle, state: State<'_, AppState>) -> Result<i64, String> {
    let src = get_source_id(&state)?;
    let site_id = get_active_site_id(&state);
    let a = get_active_adapter(&state)?;
    let (candidates, mirrors) = {
        let db = state.db.lock().unwrap();
        let followed = db.list_followed(src).map_err(|e| e.to_string())?;
        let mut candidates = Vec::new();
        for s in followed {
            if db.series_needs_genre_backfill(s.id).map_err(|e| e.to_string())? {
                candidates.push(s);
            }
        }
        (candidates, load_mirrors(&db, &site_id)?)
    };
    let total = candidates.len();
    let mut filled = 0i64;
    for (idx, s) in candidates.into_iter().enumerate() {
        emit_refresh_progress(&app, idx, total, &s.title);
        if backfill_series_genre_if_missing(&app, &state.db, &mirrors, &s, a.as_ref()).await {
            let got = {
                let db = state.db.lock().unwrap();
                !db.series_needs_genre_backfill(s.id).map_err(|e| e.to_string())?
            };
            if got {
                filled += 1;
            }
            tokio::time::sleep(std::time::Duration::from_millis(800)).await;
        }
    }
    emit_refresh_progress(&app, total, total, "Completado");
    Ok(filled)
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

/// How many series' episode-list pages to fetch concurrently in refresh().
/// Cover images stay strictly one-at-a-time regardless (see the cover-fetch
/// comment below) — this only parallelizes the plain HTML episode-list
/// fetch, not the thing CLAUDE.md specifically calls out as abuse-prone.
const REFRESH_CONCURRENCY: usize = 2;

/// How long a followed series that is *absent* from the airing listing goes
/// between episode-list rechecks. The spec drafted this as 7 days on the
/// assumption "absent from the listing = finished", but live verification
/// (2026-07-10) disproved that: the schedule listing only carries ~77 series
/// while 114 are marked airing, and two followed shows absent from it
/// (Tensei Slime T4, Ryoumin 0-nin) got real new episodes the same day. So
/// absent series are rechecked daily — new episodes there arrive at most
/// ~24h late instead of ~7 days, at the cost of one full off-listing sweep
/// per day (first refresh of the day), with every later refresh that day
/// still hitting the 1-fetch fast path.
const OFF_LISTING_RECHECK_SECS: i64 = 24 * 3600;

/// The skip decision at the heart of refresh()'s optimization A (see
/// docs/superpowers/specs/2026-07-10-scraper-performance-design.md): given a
/// followed series' fresh airing-listing metadata, does its episode-list
/// page need fetching this cycle? Pure so it can be unit-tested without a
/// scraper or DB — a bug here silently stops detecting new episodes, which
/// is the app's entire purpose.
///
/// Live-verified semantics of the card metadata (2026-07-10, real site):
/// the `.sb` badge is the **upcoming episode's number**, not the count of
/// posted episodes — 8/8 followed series that were provably up to date
/// (fetched moments earlier, no new episodes) all showed `.sb == db+1`, and
/// the fixture agrees (Liar Game: `.sb`=14, newest posted episode 13). And
/// no live card (0/77) carried a past `data-rlsdt`; just-aired series
/// either roll to next week's timestamp or leave the listing entirely.
///
/// Rules, in order:
/// - `force` (the Settings "Forzar recomprobación completa" escape hatch)
///   always fetches.
/// - `listing_scanned == false` (the airing-listing scan itself failed, so
///   there is no fresh metadata) always fetches — behave exactly like the
///   pre-skip-logic refresh rather than trusting stale signals.
/// - Off the listing: fetch only when never checked or the last check is
///   older than `OFF_LISTING_RECHECK_SECS` (see its comment — absent does
///   NOT mean finished on this site).
/// - On the listing with a past countdown: the episode aired and the card
///   hasn't rolled over — fetch, whatever the badge says.
/// - Badge present: fetch iff `badge > db+1` (site has posted something we
///   don't have). `db+1` (up to date) and `<= db` (our numbering equal or
///   ahead) both skip. Deliberately *not* the spec's literal "badge == db
///   skips": that rule assumed badge = posted count, which the live data
///   disproves — under it, every current weekly series re-fetches forever.
/// - Badge unknown (`"??"`): future countdown skips, absent fetches.
#[allow(clippy::too_many_arguments)]
fn should_fetch_series(
    force: bool,
    listing_scanned: bool,
    on_listing: bool,
    next_episode_at: Option<i64>,
    site_episode_count: Option<i64>,
    db_episode_count: i64,
    last_checked_age_secs: Option<i64>,
    now_unix: i64,
) -> bool {
    if force || !listing_scanned {
        return true;
    }
    if !on_listing {
        return match last_checked_age_secs {
            None => true,
            Some(age) => age >= OFF_LISTING_RECHECK_SECS,
        };
    }
    if next_episode_at.is_some_and(|t| t <= now_unix) {
        return true;
    }
    match site_episode_count {
        Some(next_number) => next_number > db_episode_count + 1,
        None => next_episode_at.is_none(),
    }
}

/// Fetch one series' episode list across mirrors. `None` means "skip, keep
/// cached data" (malformed stored URL, or every mirror failed/mismatched) —
/// same not-an-error semantics `refresh()`'s loop always had here.
async fn fetch_series_episodes(
    app: &AppHandle,
    mirrors: &[String],
    a: &dyn SiteAdapter,
    series_url: &str,
) -> Option<(Vec<Episode>, String, String)> {
    let path = match url::Url::parse(series_url) {
        Ok(u) => format!("{}{}", u.path(), u.query().map(|q| format!("?{q}")).unwrap_or_default()),
        Err(_) => return None,
    };
    match scrape_via_mirrors(app, mirrors, &path, false, |html| a.parse_series(html)).await {
        Ok((_scraped, eps, working_mirror)) => Some((eps, working_mirror, path)),
        Err(_) => None,
    }
}

/// For each followed series: decide (from one fresh airing-listing fetch)
/// whether its episode page can even have changed, and scrape only the ones
/// that can (falling back across mirrors), inserting new episodes. Returns
/// count of new episodes.
///
/// Optimization A of the scraper-performance design: the airing listing
/// already carries, for every series on it, the next-release timestamp
/// (`data-rlsdt`) and the site's episode count (`.sb`), so a refresh with no
/// new episodes anywhere needs 1 page fetch instead of ~119. See
/// `should_fetch_series` for the exact skip rules and their correctness
/// reasoning. `force: true` (the "Forzar recomprobación completa" Settings
/// action) ignores every skip rule — the user's escape hatch, and the
/// A/B-verification path for the skip logic itself.
#[tauri::command]
pub async fn refresh(app: AppHandle, state: State<'_, AppState>, force: bool) -> Result<i64, String> {
    let refresh_started = std::time::Instant::now();
    let src = get_source_id(&state)?;
    let site_id = get_active_site_id(&state);
    let a = get_active_adapter(&state)?;
    let mirrors = {
        let db = state.db.lock().unwrap();
        load_mirrors(&db, &site_id)?
    };

    // One fresh airing-listing fetch up front — the skip decisions below are
    // only sound against metadata from *this* scan, never a stale one, so
    // this scan deliberately doesn't consult any cache. On failure (all
    // mirrors down) fall back to fetching every followed series, exactly
    // like the pre-skip-logic refresh.
    emit_refresh_progress(&app, 0, 1, "Escaneando listado de estrenos");
    let listing_path = a.airing_url("").to_string();
    let listing_slugs: Option<std::collections::HashSet<String>> =
        match scrape_via_mirrors(&app, &mirrors, &listing_path, false, |html| a.parse_airing(html)).await {
            Ok((_scraped, series, _mirror)) => {
                let db = state.db.lock().unwrap();
                for s in &series {
                    db.upsert_series(src, s).map_err(|e| e.to_string())?;
                }
                Some(series.into_iter().map(|s| s.slug).collect())
            }
            Err(e) => {
                eprintln!("[scrape] refresh: airing-listing scan failed ({e}); falling back to fetching every followed series");
                None
            }
        };

    // Re-read followed AFTER the listing upserts so next_episode_at /
    // site_episode_count are this scan's values, not last week's.
    let followed = {
        let db = state.db.lock().unwrap();
        db.list_followed(src).map_err(|e| e.to_string())?
    };
    let total_series = followed.len();
    let now_unix = chrono::Utc::now().timestamp();

    // Partition into skip/fetch. Skipped series are reported to the progress
    // bar immediately (the bar visibly races through them) so X/Y still
    // covers ALL followed series, not just the fetched ones.
    let mut to_fetch: Vec<Series> = Vec::new();
    let mut idx = 0usize;
    for s in followed {
        let (db_count, checked_age) = {
            let db = state.db.lock().unwrap();
            (
                db.episode_count(s.id).map_err(|e| e.to_string())?,
                db.last_checked_age_secs(s.id).map_err(|e| e.to_string())?,
            )
        };
        let on_listing = listing_slugs.as_ref().is_some_and(|set| set.contains(&s.slug));
        if should_fetch_series(
            force,
            listing_slugs.is_some(),
            on_listing,
            s.next_episode_at,
            s.site_episode_count,
            db_count,
            checked_age,
            now_unix,
        ) {
            to_fetch.push(s);
        } else {
            idx += 1;
            emit_refresh_progress(&app, idx, total_series, &s.title);
        }
    }
    let skipped = total_series - to_fetch.len();
    eprintln!(
        "[scrape] refresh: {skipped} skipped, {} to fetch (force={force}, listing_scanned={})",
        to_fetch.len(),
        listing_slugs.is_some()
    );

    let mut total_new = 0i64;
    for chunk in to_fetch.chunks(REFRESH_CONCURRENCY) {
        for s in chunk {
            emit_refresh_progress(&app, idx, total_series, &s.title);
            idx += 1;
        }

        // Fetch this chunk's episode-list pages concurrently (plain HTML,
        // not the image-bulk case CLAUDE.md warns about) — everything after
        // this (DB writes, cover fetch, genre backfill) stays sequential
        // per series below, so cover images never overlap.
        let fetched: Vec<Option<(Vec<Episode>, String, String)>> = match chunk {
            [s0, s1] => {
                let (r0, r1) = tokio::join!(
                    fetch_series_episodes(&app, &mirrors, a.as_ref(), &s0.url),
                    fetch_series_episodes(&app, &mirrors, a.as_ref(), &s1.url),
                );
                vec![r0, r1]
            }
            _ => {
                let mut v = Vec::with_capacity(chunk.len());
                for s in chunk {
                    v.push(fetch_series_episodes(&app, &mirrors, a.as_ref(), &s.url).await);
                }
                v
            }
        };

        for (s, result) in chunk.iter().zip(fetched) {
            let Some((eps, working_mirror, path)) = result else { continue };
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
                // Only on a *successful* fetch — a failed one leaves the old
                // timestamp so the finished-show recheck retries next cycle.
                db.set_last_checked_at(s.id).map_err(|e| e.to_string())?;
            }

            // One cover fetch per followed series per refresh — never in
            // bulk, never concurrent with another cover fetch (this inner
            // loop is sequential). Skip once it's already a fetched data:
            // URI; a failure here just leaves the remote (broken) url in
            // place to retry next time, it never blocks episode updates.
            if let Some(remote) = &s.cover_url {
                if !remote.starts_with("data:") {
                    if let Ok(data_uri) = fetch_cover_image(&app, remote).await {
                        let db = state.db.lock().unwrap();
                        let _ = db.update_series_cover(s.id, &data_uri);
                    }
                }
            }

            // One genre/kind backfill per followed series per refresh —
            // only for series that don't already have series_genres rows.
            // A failure here is silent and never blocks episode updates.
            backfill_series_genre_if_missing(&app, &state.db, &mirrors, s, a.as_ref()).await;
        }

        // Polite delay once per chunk rather than once per series — a chunk
        // already spaces its own concurrent requests out with real work
        // (page parse, DB writes, cover fetch), so this is deliberately not
        // simply "old delay ÷ concurrency"; halving it keeps a comparable
        // pace to before while trimming the fixed idle time in half.
        tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    }
    emit_refresh_progress(&app, total_series, total_series, "Completado");
    eprintln!(
        "[scrape] refresh() wall time: {:?} for {total_series} followed series ({skipped} skipped), {total_new} new episodes",
        refresh_started.elapsed()
    );
    Ok(total_new)
}

/// Pending queue, ordered by episodes-remaining per series. `sort` is the
/// JS-side string `"remaining_asc"` | `"remaining_desc"`; anything else
/// (incl. `None`) defaults to ascending (fewest-left first).
#[tauri::command]
pub fn list_pending(
    state: State<'_, AppState>,
    sort: Option<String>,
) -> Result<Vec<PendingItem>, String> {
    let sort = match sort.as_deref() {
        Some("remaining_desc") => crate::db::PendingSort::RemainingDesc,
        _ => crate::db::PendingSort::RemainingAsc,
    };
    let src = get_source_id(&state)?;
    let db = state.db.lock().unwrap();
    let rows = db.list_pending(src, sort).map_err(|e| e.to_string())?;
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
    let src = get_source_id(&state)?;
    let db = state.db.lock().unwrap();
    db.pending_count(src).map_err(|e| e.to_string())
}

/// Open an episode or info page in a visible, app-owned WebView2 window
/// (separate from the user's browser — see `AppWindowPlayer`). Does NOT mark
/// it seen — the user marks seen/unseen explicitly via `set_seen`.
///
/// `async` on purpose: creating a WebView2 window from a *synchronous* command
/// produced a blank window that closed itself (and an earlier main-thread-
/// dispatch attempt deadlocked the UI). Async commands run on Tauri's runtime,
/// the same context `scraper_engine` builds its windows from successfully.
#[tauri::command]
pub async fn open_episode(app: AppHandle, url: String) -> Result<(), String> {
    let ep = Episode {
        id: 0,
        series_id: 0,
        number: String::new(),
        title: None,
        url,
        released_at: None,
        seen: false,
    };
    AppWindowPlayer.open(&app, &ep).map_err(|e| e.to_string())
}

/// Mark an episode seen or unseen (persisted).
#[tauri::command]
pub fn set_seen(state: State<'_, AppState>, episode_id: i64, seen: bool) -> Result<(), String> {
    let db = state.db.lock().unwrap();
    db.set_seen(episode_id, seen).map_err(|e| e.to_string())
}

/// Mark an episode seen/unseen, cascading to keep watching gap-free: marking
/// seen also marks every earlier episode of the series seen; marking unseen
/// also marks every later one unseen.
#[tauri::command]
pub fn set_seen_cascade(
    state: State<'_, AppState>,
    series_id: i64,
    number: String,
    seen: bool,
) -> Result<(), String> {
    let db = state.db.lock().unwrap();
    db.set_seen_cascade(series_id, &number, seen).map_err(|e| e.to_string())
}

/// All episodes of a series (progress view), oldest first.
#[tauri::command]
pub fn list_episodes(state: State<'_, AppState>, series_id: i64) -> Result<Vec<Episode>, String> {
    let db = state.db.lock().unwrap();
    db.list_series_episodes(series_id).map_err(|e| e.to_string())
}

#[derive(serde::Deserialize)]
pub enum SwipeDecision {
    Seen,
    Want,
    Discard,
}

/// Pick a random cached-or-fresh (genre, page) pair, scrape it if not
/// already buffered this session, filter out anything already decided,
/// shuffle, and pop one card. `Ok(None)` means the freshly-scraped page (or
/// buffer) had nothing left after filtering — a normal "everything on this
/// page was already decided" case, not an error; the frontend just calls
/// again.
///
/// Not wired to the Section B genre/format bans (`get_banned_genres`/
/// `get_banned_formats`): Descubrir.tsx only calls the catalog-deck path
/// (`discover_catalog_card`/`decide_catalog_card`) — this site-scraped swipe
/// deck has no live caller in the current UI, so extending it here would be
/// speculative scope creep. See the design spec's "Out of scope".
#[tauri::command]
pub async fn discover_swipe_card(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Option<FinishedCard>, String> {
    let src = get_source_id(&state)?;
    let site_id = get_active_site_id(&state);
    let a = get_active_adapter(&state)?;
    let mirrors = {
        let db = state.db.lock().unwrap();
        load_mirrors(&db, &site_id)?
    };
    let genres = ensure_genre_list(&app, &mirrors, &state, a.as_ref(), &site_id).await?;
    if genres.is_empty() {
        return Err("no se encontraron géneros; reintenta el escaneo".into());
    }
    // Taste-weighted genre pick: bias toward genres this user actually
    // follows/wants over ones they've discarded, instead of picking
    // uniformly at random. Falls back to uniform whenever every genre nets
    // <= 0 (nothing decided yet), so a fresh profile still discovers freely.
    let affinity = {
        let db = state.db.lock().unwrap();
        db.get_genre_affinity(src).map_err(|e| e.to_string())?
    };
    let weights: Vec<f64> = genres
        .iter()
        .map(|(_, name)| *affinity.get(name).unwrap_or(&0.0))
        .collect();
    let pick = weighted_pick_index(&weights).unwrap();
    let (slug, name) = &genres[pick];

    let last_page = state.swipe_last_page.lock().unwrap().get(slug).copied().unwrap_or(1);
    let page = pick_index(last_page as usize).map(|i| i as u32 + 1).unwrap_or(1);

    let known = {
        let db = state.db.lock().unwrap();
        db.known_series_urls(src).map_err(|e| e.to_string())?
    };

    let buffered = state.swipe_buffer.lock().unwrap().remove(&(slug.clone(), page));
    let mut cards = match buffered {
        Some(cards) => undecided_cards(cards, &known),
        None => {
            let path = a.genre_page_url("", slug, page);
            let (scraped, raw_cards, _mirror) =
                scrape_via_mirrors(&app, &mirrors, &path, true, |html| a.parse_finished_page(html)).await?;
            state
                .swipe_last_page
                .lock()
                .unwrap()
                .insert(slug.clone(), a.parse_pagination_last_page(&scraped.html));
            let mut fresh = undecided_cards(raw_cards, &known);
            shuffle(&mut fresh);
            fresh
        }
    };

    let Some(mut card) = cards.pop() else {
        return Ok(None);
    };
    if !cards.is_empty() {
        state.swipe_buffer.lock().unwrap().insert((slug.clone(), page), cards);
    }
    card.matched_genre = Some(name.clone());
    state.swipe_served.lock().unwrap().insert(card.url.clone(), card.clone());
    Ok(Some(card))
}

#[tauri::command]
pub async fn decide_swipe(
    app: AppHandle,
    state: State<'_, AppState>,
    series_url: String,
    decision: SwipeDecision,
) -> Result<(), String> {
    let src = get_source_id(&state)?;
    let site_id = get_active_site_id(&state);
    let a = get_active_adapter(&state)?;
    let card = state
        .swipe_served
        .lock()
        .unwrap()
        .remove(&series_url)
        .ok_or_else(|| "unknown swipe card; call discover_swipe_card first".to_string())?;

    let series = Series {
        id: 0,
        slug: slug_from_url(&card.url),
        title: card.title.clone(),
        url: card.url.clone(),
        cover_url: card.poster_url.clone(),
        is_airing: false,
        followed: false, next_episode_at: None, site_episode_count: None,
    };

    let sid = match decision {
        SwipeDecision::Discard => {
            let db = state.db.lock().unwrap();
            let sid = db.upsert_series(src, &series).map_err(|e| e.to_string())?;
            db.set_kind(sid, &card.kind).map_err(|e| e.to_string())?;
            db.set_backlog_status(sid, Some("discarded")).map_err(|e| e.to_string())?;
            sid
        }
        SwipeDecision::Want => {
            let mirrors = {
                let db = state.db.lock().unwrap();
                load_mirrors(&db, &site_id)?
            };
            let detail = fetch_series_detail(&app, &mirrors, &card.url, a.as_ref()).await?;
            let db = state.db.lock().unwrap();
            let sid = db.upsert_series(src, &series).map_err(|e| e.to_string())?;
            db.set_kind(sid, detail.kind.as_deref().unwrap_or(&card.kind)).map_err(|e| e.to_string())?;
            db.insert_series_genres(sid, &detail.genres).map_err(|e| e.to_string())?;
            db.set_backlog_status(sid, Some("want")).map_err(|e| e.to_string())?;
            sid
        }
        SwipeDecision::Seen => {
            let mirrors = {
                let db = state.db.lock().unwrap();
                load_mirrors(&db, &site_id)?
            };
            let detail = fetch_series_detail(&app, &mirrors, &card.url, a.as_ref()).await?;
            let eps = fetch_episode_list_for(&app, &mirrors, &card.url, a.as_ref()).await?;
            let db = state.db.lock().unwrap();
            let sid = db.upsert_series(src, &series).map_err(|e| e.to_string())?;
            db.set_followed(sid, true).map_err(|e| e.to_string())?;
            db.set_kind(sid, detail.kind.as_deref().unwrap_or(&card.kind)).map_err(|e| e.to_string())?;
            db.insert_series_genres(sid, &detail.genres).map_err(|e| e.to_string())?;
            for mut e in eps {
                e.series_id = sid;
                e.seen = true;
                db.insert_episode(&e).map_err(|e| e.to_string())?;
            }
            sid
        }
    };
    push_swipe_history(&state, sid);
    Ok(())
}

/// Undo the most recent decide_swipe/decide_catalog_card call (Ctrl+Z) by
/// popping it off the front of `swipe_history` and hard-deleting the series
/// row it created. Calling this with nothing left to undo is a no-op, not an
/// error. For reaching further back than just the most recent, see
/// `undo_swipe_entry`.
#[tauri::command]
pub fn undo_last_swipe(state: State<'_, AppState>) -> Result<(), String> {
    let sid = state.swipe_history.lock().unwrap().pop_front();
    if let Some(sid) = sid {
        let db = state.db.lock().unwrap();
        db.delete_series(sid).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// One entry in the swipe-history strip — a still-live `series` row from
/// `swipe_history`, with its decision derived live from the row's current
/// classification flags (not stored separately, so a `reclassify_series`
/// call in between is reflected automatically on the next read).
#[derive(Serialize, Clone)]
pub struct SwipeHistoryItem {
    pub series_id: i64,
    pub title: String,
    pub poster_url: Option<String>,
    /// "seen" | "want" | "discard" | "none", derived from the row's live
    /// `watched_externally`/`backlog_status` — see `decision_for_history_row`.
    pub decision: String,
}

fn decision_for_history_row(row: &crate::db::SwipeHistoryRow) -> String {
    if row.watched_externally {
        "seen".to_string()
    } else {
        match row.backlog_status.as_deref() {
            Some("want") => "want".to_string(),
            Some("discarded") => "discard".to_string(),
            _ => "none".to_string(),
        }
    }
}

/// Up to `SWIPE_HISTORY_CAP` most-recent swipe decisions still live in the
/// DB, most-recent first — the Descubrir history strip's data source. An id
/// in `swipe_history` whose row was already deleted (a prior undo/return-to-
/// deck) is silently skipped rather than erroring, so the deque self-heals
/// instead of requiring bookkeeping on every deletion path.
#[tauri::command]
pub fn list_swipe_history(state: State<'_, AppState>) -> Result<Vec<SwipeHistoryItem>, String> {
    let ids: Vec<i64> = state.swipe_history.lock().unwrap().iter().copied().collect();
    let db = state.db.lock().unwrap();
    let mut out = Vec::with_capacity(ids.len());
    for sid in ids {
        if let Some(row) = db.get_series_for_history(sid).map_err(|e| e.to_string())? {
            out.push(SwipeHistoryItem {
                series_id: sid,
                title: row.title.clone(),
                poster_url: row.poster_url.clone(),
                decision: decision_for_history_row(&row),
            });
        }
    }
    Ok(out)
}

/// Return a specific past card to the deck: hard-delete its series row (so
/// the catalog picker's `anilist_id NOT IN (...)` exclusion no longer
/// applies) and drop it from `swipe_history`. Unlike `undo_last_swipe`, this
/// targets an arbitrary entry in the history strip, not just the front —
/// "undo this one" rather than "undo my last action". A no-op (not an
/// error) if `series_id` isn't in the history or its row is already gone.
#[tauri::command]
pub fn undo_swipe_entry(state: State<'_, AppState>, series_id: i64) -> Result<(), String> {
    state.swipe_history.lock().unwrap().retain(|&id| id != series_id);
    let db = state.db.lock().unwrap();
    db.delete_series(series_id).map_err(|e| e.to_string())?;
    Ok(())
}

/// Series with the given backlog status ('want' or 'discarded'), for the
/// swipe mode's "Listas" sub-view.
#[tauri::command]
pub fn list_backlog(state: State<'_, AppState>, status: String) -> Result<Vec<Series>, String> {
    let src = get_source_id(&state)?;
    let db = state.db.lock().unwrap();
    db.list_backlog(src, &status).map_err(|e| e.to_string())
}

/// Series marked "watched outside the app" (the catalog "Ya lo vi" swipe),
/// for the Listas view's "Ya vistas" sub-list — mirrors `list_backlog`.
#[tauri::command]
pub fn list_watched_externally(state: State<'_, AppState>) -> Result<Vec<Series>, String> {
    let src = get_source_id(&state)?;
    let db = state.db.lock().unwrap();
    db.list_watched_externally(src).map_err(|e| e.to_string())
}

/// Move a discarded row back to 'want'. If it never had its detail page
/// fetched (the common case — discard never fetches), fetch it now so the
/// row has genres before it shows up as a normal "want" backlog item; if it
/// already has genres (e.g. a previously-"want" row that got discarded),
/// just flip the status.
#[tauri::command]
pub async fn promote_discarded(
    app: AppHandle,
    state: State<'_, AppState>,
    series_id: i64,
) -> Result<(), String> {
    let site_id = get_active_site_id(&state);
    let a = get_active_adapter(&state)?;
    let (needs_detail, series_url, mirrors) = {
        let db = state.db.lock().unwrap();
        let needs_detail = db.series_needs_genre_backfill(series_id).map_err(|e| e.to_string())?;
        let url = db
            .get_series_url(series_id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "series not found".to_string())?;
        (needs_detail, url, load_mirrors(&db, &site_id)?)
    };
    if needs_detail {
        let detail = fetch_series_detail(&app, &mirrors, &series_url, a.as_ref()).await?;
        let db = state.db.lock().unwrap();
        db.insert_series_genres(series_id, &detail.genres).map_err(|e| e.to_string())?;
        if let Some(kind) = &detail.kind {
            db.set_kind(series_id, kind).map_err(|e| e.to_string())?;
        }
    }
    let db = state.db.lock().unwrap();
    db.set_backlog_status(series_id, Some("want")).map_err(|e| e.to_string())?;
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

/// Promote a `backlog_status='want'` row to an ordinary followed series.
///
/// Two shapes of "want" row reach here:
///
/// - A **catalog** row (synthetic `anilist-{id}` slug, AniList URL, no
///   episodes): its `url` points at anilist.co, so there is nothing to scrape
///   there. It must be **linked to the real site first** — this is one of the
///   three (and only three) explicit link triggers in the design spec, fired
///   because the user pressed "Empezar a ver". If the matcher can't find it on
///   the site (`NoMatch`), the row is left untouched in the backlog and the
///   outcome is returned so the UI can show "No encontrado" instead of
///   silently creating a followed-but-untrackable series. Linking already
///   scrapes and inserts the episode list, so no separate episode fetch runs.
/// - A **site** row (real slug/URL from a `decide_swipe` Want, `anilist_id`
///   NULL): unchanged behaviour — fetch its episode list, follow, clear the
///   backlog status.
///
/// `refresh()` already scans all followed rows regardless of `is_airing`, so
/// no changes are needed there. Returns a `LinkOutcome` so the caller can
/// distinguish a successful start from a `NoMatch` that left the row as-is.
#[tauri::command]
pub async fn start_watching(
    app: AppHandle,
    state: State<'_, AppState>,
    series_id: i64,
) -> Result<LinkOutcome, String> {
    let info = {
        let db = state.db.lock().unwrap();
        let status = db.get_backlog_status(series_id).map_err(|e| e.to_string())?;
        if status.as_deref() != Some("want") {
            return Err("series is not in the 'want' backlog".into());
        }
        db.get_series_for_link(series_id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "series not found".to_string())?
    };

    // Catalog row (anilist-backed) with no real site episodes yet: link it
    // before following. A NoMatch leaves the row exactly as it is.
    let has_episodes = {
        let db = state.db.lock().unwrap();
        !db.list_series_episodes(series_id).map_err(|e| e.to_string())?.is_empty()
    };
    if info.anilist_id.is_some() && !has_episodes {
        let (outcome, resulting_id) = link_series_core(&app, &state, series_id).await?;
        let Some(target_id) = resulting_id else {
            // NoMatch — do not follow; hand the outcome back so the UI can
            // surface "No encontrado" and keep the row in the backlog.
            return Ok(outcome);
        };
        let db = state.db.lock().unwrap();
        db.set_followed(target_id, true).map_err(|e| e.to_string())?;
        db.set_backlog_status(target_id, None).map_err(|e| e.to_string())?;
        return Ok(outcome);
    }

    // Site row: fetch its episode list (all unseen), follow, clear backlog.
    let site_id = get_active_site_id(&state);
    let a = get_active_adapter(&state)?;
    let (series_url, mirrors) = {
        let db = state.db.lock().unwrap();
        let url = db
            .get_series_url(series_id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "series not found".to_string())?;
        (url, load_mirrors(&db, &site_id)?)
    };
    let eps = fetch_episode_list_for(&app, &mirrors, &series_url, a.as_ref()).await?;
    let db = state.db.lock().unwrap();
    let episode_count = eps.len() as i64;
    for mut e in eps {
        e.series_id = series_id;
        e.seen = false;
        db.insert_episode(&e).map_err(|e| e.to_string())?;
    }
    db.set_followed(series_id, true).map_err(|e| e.to_string())?;
    db.set_backlog_status(series_id, None).map_err(|e| e.to_string())?;
    Ok(LinkOutcome::Linked { url: series_url, episodes: episode_count })
}

/// Set (or clear, with `status: None`) a series' backlog status directly —
/// used by the Listas view's "Descartar" action on a "want" row, which
/// (unlike decide_swipe) is transitioning a series already in the DB, not
/// inserting a fresh one from a swipe card.
#[tauri::command]
pub fn set_backlog_status(
    state: State<'_, AppState>,
    series_id: i64,
    status: Option<String>,
) -> Result<(), String> {
    let db = state.db.lock().unwrap();
    db.set_backlog_status(series_id, status.as_deref()).map_err(|e| e.to_string())
}

/// The four non-scraping classification states a series can be moved
/// between — see the state table in
/// docs/superpowers/specs/2026-07-11-reversibility-classifications-design.md.
/// Plain Rust variant names on the wire (no serde rename), same convention
/// as `SwipeDecision`.
#[derive(serde::Deserialize)]
pub enum Classification {
    None,
    Want,
    Discarded,
    WatchedExternally,
}

/// The body of `reclassify_series`, factored out (in the `switch_site_core`
/// style) so it can be unit-tested against an in-memory `Db` without a
/// `State<AppState>`. Clears all three classification signals
/// (`followed`/`backlog_status`/`watched_externally`) then applies `to` —
/// reusing the existing `db.set_*` methods rather than raw SQL, so this
/// stays in sync with their semantics. Deliberately does **not** scrape and
/// does **not** touch `episodes`/seen rows: the one target this can't reach
/// (Want -> actively Watching for a catalog stub with no episodes) needs a
/// scrape and stays on `start_watching`, unchanged.
fn reclassify_series_core(db: &Db, series_id: i64, to: Classification) -> Result<(), String> {
    db.set_followed(series_id, false).map_err(|e| e.to_string())?;
    db.set_backlog_status(series_id, None).map_err(|e| e.to_string())?;
    db.set_watched_externally(series_id, false).map_err(|e| e.to_string())?;
    match to {
        Classification::None => {}
        Classification::Want => {
            db.set_backlog_status(series_id, Some("want")).map_err(|e| e.to_string())?
        }
        Classification::Discarded => {
            db.set_backlog_status(series_id, Some("discarded")).map_err(|e| e.to_string())?
        }
        Classification::WatchedExternally => {
            db.set_watched_externally(series_id, true).map_err(|e| e.to_string())?
        }
    }
    Ok(())
}

/// Move a series between the four non-scraping classification states in one
/// atomic step — the universal "de-classify / move between lists" inverse
/// (Library "Dejar de seguir"/"Mover a Quiero ver", SeriesDetail "Dejar de
/// seguir", Descubrir Listas "Quitar de la lista" and the "Ya vistas"
/// sub-list). Held under a single `state.db` lock so nothing else can
/// interleave a partial state. Never scrapes, never touches episodes/seen.
#[tauri::command]
pub fn reclassify_series(
    state: State<'_, AppState>,
    series_id: i64,
    to: Classification,
) -> Result<(), String> {
    let db = state.db.lock().unwrap();
    reclassify_series_core(&db, series_id, to)
}

/// Full genre list for one series, for the Listas view's "Quiero ver" rows.
#[tauri::command]
pub fn get_series_genres(state: State<'_, AppState>, series_id: i64) -> Result<Vec<String>, String> {
    let db = state.db.lock().unwrap();
    db.list_series_genres(series_id).map_err(|e| e.to_string())
}

/// This user's top `limit` genres by affinity score (positive only), for
/// the "Tus géneros favoritos" chip row in the swipe UI — makes the
/// otherwise-invisible taste-weighting in `discover_swipe_card` visible.
#[tauri::command]
pub fn get_top_genres(state: State<'_, AppState>, limit: usize) -> Result<Vec<GenreAffinity>, String> {
    let src = get_source_id(&state)?;
    let db = state.db.lock().unwrap();
    let mut scored: Vec<(String, f64)> = db
        .get_genre_affinity(src)
        .map_err(|e| e.to_string())?
        .into_iter()
        .filter(|(_, score)| *score > 0.0)
        .collect();
    // Tie-break alphabetically: scores come out of a HashMap, so without a
    // secondary key equal-scored genres would reorder from call to call.
    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal).then_with(|| a.0.cmp(&b.0)));
    scored.truncate(limit);
    Ok(scored.into_iter().map(|(genre, score)| GenreAffinity { genre, score }).collect())
}

#[derive(Serialize)]
pub struct CatalogPage {
    pub items: Vec<crate::anilist::CatalogAnime>,
    pub has_next_page: bool,
    pub total_synced: i64,
    pub total_matching: i64,
}

/// One page of the locally-synced AniList catalog — the "Catálogo" tab's
/// data source, independent of anything scraped off the tracked site. Reads
/// the local `anilist_catalog` table (see `sync_anime_catalog`), not
/// AniList live — instant, no rate-limit exposure, works with whatever's
/// been synced so far even mid-sync. `filter` narrows the result (search,
/// genres, format, score, episode bucket — see `crate::db::CatalogFilter`);
/// `total_synced` stays the unfiltered grand total (header text) while
/// `total_matching` reflects `filter` and drives `has_next_page` / the
/// "N resultados" label.
#[tauri::command]
pub fn get_anime_catalog(
    state: State<'_, AppState>,
    page: i64,
    filter: Option<crate::db::CatalogFilter>,
) -> Result<CatalogPage, String> {
    let db = state.db.lock().unwrap();
    let per_page = 30;
    let filter = filter.unwrap_or_default();
    let items = db.list_catalog_filtered(page, per_page, &filter).map_err(|e| e.to_string())?;
    let total_synced = db.catalog_count().map_err(|e| e.to_string())?;
    let total_matching = db.catalog_count_filtered(&filter).map_err(|e| e.to_string())?;
    let has_next_page = page * per_page < total_matching;
    Ok(CatalogPage { items, has_next_page, total_synced, total_matching })
}

#[derive(Serialize)]
pub struct CatalogFacets {
    pub genres: Vec<String>,
    pub formats: Vec<String>,
}

/// Distinct genre/format vocabularies from the synced catalog — drives the
/// Catálogo filter bar's chips/select without hardcoding AniList's own
/// vocabulary (which can grow over time). Cheap `SELECT DISTINCT`s; the
/// frontend loads this once per mount, tolerating staleness until the next
/// sync (facets only change after a re-sync).
#[tauri::command]
pub fn get_catalog_facets(state: State<'_, AppState>) -> Result<CatalogFacets, String> {
    let db = state.db.lock().unwrap();
    let genres = db.distinct_catalog_genres().map_err(|e| e.to_string())?;
    let formats = db.distinct_catalog_formats().map_err(|e| e.to_string())?;
    Ok(CatalogFacets { genres, formats })
}

/// The Descubrir deck's user-configured genre/format bans (Section B) — see
/// `db::{get_banned_genres, get_banned_formats}`. Global (not per-site).
#[derive(Serialize, Clone)]
pub struct DeckBans {
    pub genres: Vec<String>,
    pub formats: Vec<String>,
}

/// Read the current deck bans for the Descubrir "Filtros" sub-view.
#[tauri::command]
pub fn get_deck_bans(state: State<'_, AppState>) -> Result<DeckBans, String> {
    let db = state.db.lock().unwrap();
    let genres = db.get_banned_genres().map_err(|e| e.to_string())?;
    let formats = db.get_banned_formats().map_err(|e| e.to_string())?;
    Ok(DeckBans { genres, formats })
}

/// Persist the deck bans. Takes effect on the very next `discover_catalog_card`
/// call — no cache to invalidate, `discover_catalog_card` reads the settings
/// table fresh every time it's called.
#[tauri::command]
pub fn set_deck_bans(state: State<'_, AppState>, genres: Vec<String>, formats: Vec<String>) -> Result<(), String> {
    let db = state.db.lock().unwrap();
    db.set_banned_genres(&genres).map_err(|e| e.to_string())?;
    db.set_banned_formats(&formats).map_err(|e| e.to_string())?;
    Ok(())
}

#[derive(Serialize, Clone)]
struct CatalogSyncProgress {
    synced: i64,
    total: i64,
}

/// Fetch AniList's full anime catalog and store it locally. Replaces a
/// single popularity-sorted crawl (which silently capped out around 5000
/// titles — AniList hard-limits `page * perPage <= 5000`, and
/// `pageInfo.total`/`lastPage` always report a fake fixed value, verified
/// live) with a list of date/status **partitions** each guaranteed to stay
/// under that cap, crawled to exhaustion via the one reliable signal,
/// `pageInfo.hasNextPage` — see `anilist::build_partitions` for the
/// partition scheme and why (year buckets, not `seasonYear` or a cursor
/// that doesn't exist on this API).
///
/// State (`settings` key `catalog_sync_state`) persists which partitions
/// have finished so an interrupted sync resumes instead of restarting, and
/// once a full sync has completed once, subsequent syncs only re-run the
/// window where titles actually change (current year ± a couple, plus
/// upcoming) — see `anilist::{build_partitions, incremental_partitions}`.
/// `force_full=true` clears that state and redoes everything from scratch.
///
/// Pacing: paces almost every request (~2.1s apart, ~28.6 req/min) and only
/// skips the sleep for the handful of requests right after AniList's
/// rate-limit window (`X-Ratelimit-Remaining`, 30/min unauthenticated) has
/// just reset. An earlier version skipped sleeping whenever `remaining >= 3`
/// to "burst through headroom" — live testing (2026-07-10) showed this
/// blows through most of a window in a few seconds of request/response
/// round-trips, then stays rate-limited for the rest of that ~60s window
/// regardless of per-request sleep length (the window doesn't refill
/// per-request like a token bucket), which surfaced as a partition failing
/// after exhausting its 429 retries. A 429 is still honored via
/// `Retry-After` inside `fetch_partition_page` as a safety net.
///
/// This is a real multi-minute job for the ~21,000-title full catalog, so
/// it's user-triggered from the Catálogo tab, not run automatically on
/// every app launch.
#[tauri::command]
pub async fn sync_anime_catalog(
    app: AppHandle,
    state: State<'_, AppState>,
    force_full: bool,
) -> Result<i64, String> {
    use crate::anilist::{build_partitions, incremental_partitions, pending_partitions, CatalogSyncState};
    use chrono::Datelike;

    const PAGE_SIZE: i64 = 50;
    // Real total (~21,000) can't be known up front (AniList's own totals
    // are fake) — this is a stable baseline for the progress bar's `max`,
    // not a claim of precision. Clamped up if synced ever exceeds it so the
    // bar can't show >100%.
    const FULL_SYNC_ESTIMATE: i64 = 21_000;
    // Only skip the pacing sleep when headroom is still near a fully-fresh
    // window (>= this many of the 30 requests/min still available) — a live
    // run (2026-07-10) with a low burst threshold (skip-sleep whenever
    // remaining >= 3) blew through a whole window in a few seconds of RTT
    // and then stayed rate-limited for the next ~50s of the rolling window
    // regardless of pacing, because AniList's window doesn't refill per
    // request the way a token bucket would — it's closer to a rolling
    // 60s window, so burning most of it in a handful of seconds means the
    // next ~30 requests are blocked no matter how long a *per-request*
    // sleep is, until that whole window ages out. Raising the floor close
    // to the window size means we pace almost every request (~2.1s apart,
    // ~28.6 req/min, safely under the 30/min cap) and only skip sleeping
    // for the handful of requests right after a window resets.
    const RATE_HEADROOM_FLOOR: i64 = 27;
    const PACED_SLEEP: std::time::Duration = std::time::Duration::from_millis(2100);

    let current_year = chrono::Utc::now().year();

    let mut sync_state = if force_full {
        CatalogSyncState::default()
    } else {
        let db = state.db.lock().unwrap();
        db.get_setting("catalog_sync_state")
            .map_err(|e| e.to_string())?
            .map(|raw| CatalogSyncState::from_json(&raw))
            .unwrap_or_default()
    };

    let is_incremental = sync_state.full_sync_completed_at.is_some();
    let partitions_to_run: Vec<_> = if is_incremental {
        incremental_partitions(current_year)
    } else {
        pending_partitions(&build_partitions(current_year), &sync_state.completed)
    };

    let mut synced = {
        let db = state.db.lock().unwrap();
        db.catalog_count().map_err(|e| e.to_string())?
    };

    for partition in &partitions_to_run {
        let mut page = 1i64;
        loop {
            let result = crate::anilist::fetch_partition_page(partition, page, PAGE_SIZE)
                .await
                .map_err(|e| e.to_string())?;

            {
                let db = state.db.lock().unwrap();
                for (idx, anime) in result.items.iter().enumerate() {
                    let sort_order = synced + idx as i64;
                    db.upsert_catalog_anime(anime, sort_order).map_err(|e| e.to_string())?;
                }
                synced = db.catalog_count().map_err(|e| e.to_string())?;
            }
            let total = FULL_SYNC_ESTIMATE.max(synced);
            let _ = app.emit("catalog-sync-progress", CatalogSyncProgress { synced, total });

            let reached_page_cap = partition.max_pages.is_some_and(|cap| page >= cap);
            if !result.has_next_page || reached_page_cap {
                break;
            }
            page += 1;
            let should_pace = result.rate_remaining.map(|r| r < RATE_HEADROOM_FLOOR).unwrap_or(true);
            if should_pace {
                tokio::time::sleep(PACED_SLEEP).await;
            }
        }

        // Only a from-scratch/resuming full sync tracks per-partition
        // completion — incremental syncs always re-run their fixed window
        // and don't consult this list, so there's nothing useful to record.
        if !is_incremental {
            sync_state.completed.push(partition.label.clone());
            let db = state.db.lock().unwrap();
            db.set_setting("catalog_sync_state", &sync_state.to_json())
                .map_err(|e| e.to_string())?;
        }
    }

    if !is_incremental {
        sync_state.full_sync_completed_at = Some(chrono::Utc::now().to_rfc3339());
        let db = state.db.lock().unwrap();
        db.set_setting("catalog_sync_state", &sync_state.to_json())
            .map_err(|e| e.to_string())?;
    }

    let _ = app.emit("catalog-sync-progress", CatalogSyncProgress { synced, total: synced });
    Ok(synced)
}

/// Genres never offered by the catalog swipe deck, regardless of taste
/// weighting — hardcoded, not a setting (see spec). `Hentai` alone is 1,652
/// of the ~22,400 synced rows (~7%); left in, a uniform-ish deck would show
/// it disproportionately often relative to how any real user wants to
/// browse. `Ecchi` is excluded alongside it for the same reason (adjacent
/// content policy, not a taste signal worth surfacing here).
const EXCLUDED_CATALOG_GENRES: &[&str] = &["Hentai", "Ecchi"];

/// Bounded retry count for `discover_catalog_card`'s genre pick: if the
/// weighted-picked genre turns out to have no undecided candidate left
/// (everything in it already decided), try the next-best genre instead of
/// immediately declaring the deck exhausted.
const MAX_GENRE_ATTEMPTS: usize = 5;

/// Genres eligible for the catalog deck: every synced genre except the
/// always-on baseline (`EXCLUDED_CATALOG_GENRES` — Hentai/Ecchi) and the
/// user's own banned-genre list (Section B, `get_banned_genres`), unioned.
/// The baseline can't be lifted by a user setting; bans are strictly
/// additive to it. Factored out of `discover_catalog_card` so the filter is
/// unit-testable without a `State<AppState>`.
fn filter_candidate_genres(all_genres: Vec<String>, banned_genres: &[String]) -> Vec<String> {
    all_genres
        .into_iter()
        .filter(|g| !EXCLUDED_CATALOG_GENRES.iter().any(|ex| ex.eq_ignore_ascii_case(g)))
        .filter(|g| !banned_genres.iter().any(|b| b.eq_ignore_ascii_case(g)))
        .collect()
}

/// Pick a taste-weighted genre (mirroring `discover_swipe_card`'s scheme:
/// `get_genre_affinity` + `weighted_pick_index`, uniform fallback when
/// nothing's been decided yet) and ask the DB for a random undecided,
/// quality-floored catalog entry in it. Local + instant (no live AniList
/// call per swipe, so no rate-limit exposure from normal browsing). Catalog
/// cards carry no episode data (AniList is metadata-only), so they're
/// decided through `decide_catalog_card` rather than `decide_swipe`, which
/// assumes a scraped-site URL it can fetch an episode list from.
///
/// `Ok(None)` means the deck is genuinely exhausted: every candidate genre
/// (after excluding Hentai/Ecchi) either has zero synced titles passing the
/// quality floor or every one of them has already been decided, and
/// `MAX_GENRE_ATTEMPTS` genre picks in a row all came up empty.
#[tauri::command]
pub fn discover_catalog_card(state: State<'_, AppState>) -> Result<Option<FinishedCard>, String> {
    let src = get_source_id(&state)?;
    let db = state.db.lock().unwrap();

    let affinity = db.get_genre_affinity(src).map_err(|e| e.to_string())?;
    // Candidate-genre filter is EXCLUDED_CATALOG_GENRES (always-on baseline:
    // Hentai/Ecchi) union the user's own banned-genre list (Section B) — the
    // baseline can't be lifted by a user setting, bans are additive to it.
    let banned_genres = db.get_banned_genres().map_err(|e| e.to_string())?;
    let banned_formats = db.get_banned_formats().map_err(|e| e.to_string())?;
    let candidates = filter_candidate_genres(
        db.distinct_catalog_genres().map_err(|e| e.to_string())?,
        &banned_genres,
    );
    if candidates.is_empty() {
        return Ok(None);
    }

    // Pool of candidate-genre indices still worth trying this call; a genre
    // that yields no undecided candidate is removed from the pool so a
    // retry never re-picks the same exhausted genre.
    let mut pool: Vec<usize> = (0..candidates.len()).collect();
    for _ in 0..MAX_GENRE_ATTEMPTS.min(candidates.len()) {
        if pool.is_empty() {
            break;
        }
        let pool_weights: Vec<f64> = pool
            .iter()
            .map(|&i| *affinity.get(&candidates[i]).unwrap_or(&0.0))
            .collect();
        let Some(pick_in_pool) = weighted_pick_index(&pool_weights) else { break };
        let genre_idx = pool[pick_in_pool];
        let genre = &candidates[genre_idx];

        if let Some(anime) = db.random_catalog_anime_in_genre(genre, &banned_formats).map_err(|e| e.to_string())? {
            return Ok(Some(FinishedCard {
                title: anime.title,
                url: anime.url,
                poster_url: anime.cover_url,
                kind: anime.format.unwrap_or_default(),
                matched_genre: Some(genre.clone()),
            }));
        }
        pool.remove(pick_in_pool);
    }
    Ok(None)
}

/// Decide on a catalog-sourced card: Discard, Want, or Seen. Stores a
/// `series` row keyed by a synthetic `anilist-{id}` slug so it coexists with
/// scraped-site rows without colliding, and records the real numeric
/// `anilist_id` too (see its column comment in `db::init_schema`) so the
/// catalog picker can exclude it directly. Pushes onto `swipe_history` the
/// same way `decide_swipe` does so `undo_last_swipe`/`list_swipe_history`/
/// `undo_swipe_entry` all work uniformly across both sources.
///
/// `Seen` means "I've watched this outside the app" — AniList has no
/// episode list to mark, so unlike `decide_swipe`'s `Seen` (which fetches
/// and marks every episode watched) this just sets `watched_externally=1`
/// and clears `followed`/`backlog_status`, so the title is excluded from
/// future decks without pretending we have real watch progress for it.
#[tauri::command]
pub fn decide_catalog_card(
    state: State<'_, AppState>,
    anilist_id: i64,
    title: String,
    url: String,
    poster_url: Option<String>,
    genres: Vec<String>,
    format: String,
    decision: SwipeDecision,
) -> Result<i64, String> {
    let src = get_source_id(&state)?;
    let db = state.db.lock().unwrap();
    let series = Series {
        id: 0,
        slug: format!("anilist-{anilist_id}"),
        title,
        url,
        cover_url: poster_url,
        is_airing: false,
        followed: false, next_episode_at: None, site_episode_count: None,
    };
    let sid = db.upsert_series(src, &series).map_err(|e| e.to_string())?;
    db.set_anilist_id(sid, anilist_id).map_err(|e| e.to_string())?;
    db.set_kind(sid, &format).map_err(|e| e.to_string())?;
    db.insert_series_genres(sid, &genres).map_err(|e| e.to_string())?;
    match decision {
        SwipeDecision::Discard => {
            db.set_backlog_status(sid, Some("discarded")).map_err(|e| e.to_string())?;
        }
        SwipeDecision::Want => {
            db.set_backlog_status(sid, Some("want")).map_err(|e| e.to_string())?;
        }
        SwipeDecision::Seen => {
            db.set_backlog_status(sid, None).map_err(|e| e.to_string())?;
            db.set_followed(sid, false).map_err(|e| e.to_string())?;
            db.set_watched_externally(sid, true).map_err(|e| e.to_string())?;
        }
    }
    push_swipe_history(&state, sid);
    Ok(sid)
}

/// One search-results page, wrapped so a genuine "zero hits" answer can be
/// told apart from "this mirror's page didn't parse at all". See
/// `search_site`'s doc comment for why this indirection exists.
struct SearchOutcome {
    cards: Vec<FinishedCard>,
}

/// Search the configured site (with mirror fallback) for `query`.
///
/// Deliberately does **not** reuse `scrape_via_mirrors`'s "parsed empty ->
/// try next mirror" semantics as-is: for every other scrape in this codebase,
/// an empty parse legitimately signals "wrong/incompatible mirror", because
/// there's always *something* to find on a real airing/genre/series page.
/// A search is different — zero results for a query is a completely normal,
/// correct answer, and treating it as mirror failure would make `search_site`
/// silently keep trying (and eventually exhaust) every configured mirror any
/// time a title genuinely isn't on the site, plus misreport a real failure as
/// "not found" instead of surfacing the actual error.
///
/// The fix: wrap the parsed cards in a one-element `Vec<SearchOutcome>` before
/// handing it to `scrape_via_mirrors`. That vec is non-empty (so
/// `scrape_via_mirrors` accepts the mirror as "worked") whether the page had
/// 0 or 50 result cards on it — only `parse_search_results` itself returning
/// `Err` (page doesn't look like this site's layout at all) makes
/// `scrape_via_mirrors` fall through to the next mirror.
async fn search_site(
    app: &AppHandle,
    mirrors: &[String],
    query: &str,
    a: &dyn SiteAdapter,
) -> Result<Vec<FinishedCard>, String> {
    let path = a.search_url("", query);
    let (_scraped, mut outcomes, _mirror) = scrape_via_mirrors(app, mirrors, &path, true, |html| {
        a.parse_search_results(html).map(|cards| vec![SearchOutcome { cards }])
    })
    .await?;
    Ok(outcomes.pop().map(|o| o.cards).unwrap_or_default())
}

/// Result of `link_catalog_series` — serde-tagged so the frontend can
/// distinguish "linked, here's the episode count", "searched but nothing
/// cleared the match threshold", and "this row wasn't a synthetic catalog
/// row to begin with" without stringly-typed error parsing.
#[derive(Serialize)]
#[serde(tag = "type")]
pub enum LinkOutcome {
    Linked { url: String, episodes: i64 },
    NoMatch,
    AlreadyLinked,
}

fn to_candidates(cards: &[FinishedCard]) -> Vec<crate::matching::TitleCandidate<'_>> {
    cards.iter().map(|c| crate::matching::TitleCandidate { title: &c.title, url: &c.url }).collect()
}

/// Search the site for a synthetic catalog-swipe row's title and, on a
/// confident match, rewrite the row into a real, fully-tracked series (real
/// site URL/slug, scraped episode list, site genres/kind) — see the design
/// spec (`docs/superpowers/specs/2026-07-10-discover-site-link-search-design.md`)
/// for the full rationale. Politeness: at most 3 scrapes total (search,
/// optionally a second search with the English title, then one fetch of the
/// matched series page reused for both `parse_series` and
/// `parse_series_detail`) — no batching, no background sweep of the existing
/// backlog (out of scope, see the spec).
#[tauri::command]
pub async fn link_catalog_series(
    app: AppHandle,
    state: State<'_, AppState>,
    series_id: i64,
) -> Result<LinkOutcome, String> {
    let (outcome, _resulting_id) = link_series_core(&app, &state, series_id).await?;
    Ok(outcome)
}

/// The shared body behind both `link_catalog_series` (fire-and-forget from a
/// `Seen` swipe / manual retry) and `start_watching` (which must link an
/// unlinked catalog row *before* following it). Returns the outcome plus the
/// canonical row id the caller should act on afterwards — that's the same
/// `series_id` for an in-place relink or `AlreadyLinked`, the *existing*
/// row's id when a slug collision merged the synthetic row away (the
/// synthetic `series_id` no longer exists after that), and `None` for
/// `NoMatch` (nothing to follow).
async fn link_series_core(
    app: &AppHandle,
    state: &State<'_, AppState>,
    series_id: i64,
) -> Result<(LinkOutcome, Option<i64>), String> {
    let site_id = get_active_site_id(state);
    let a = get_active_adapter(state)?;
    let (mirrors, info) = {
        let db = state.db.lock().unwrap();
        let mirrors = load_mirrors(&db, &site_id)?;
        let info = db
            .get_series_for_link(series_id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "series not found".to_string())?;
        (mirrors, info)
    };
    // See SeriesForLink::already_linked_to_site's doc comment for why this
    // checks more than just `anilist_id.is_some()`.
    if info.already_linked_to_site() {
        return Ok((LinkOutcome::AlreadyLinked, Some(series_id)));
    }
    let anilist_id = info.anilist_id.unwrap();
    let (title, romaji, english) = {
        let db = state.db.lock().unwrap();
        db.get_catalog_titles(anilist_id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "catalog entry not found for this anilist_id".to_string())?
    };

    let primary_query = romaji.clone().unwrap_or_else(|| title.clone());
    let mut cards = search_site(app, &mirrors, &primary_query, a.as_ref()).await?;
    let mut best = crate::matching::best_match(&[&primary_query], &to_candidates(&cards));

    // Second search only on failure, only if an english title exists and
    // actually differs from what was just tried — one extra scrape, not two
    // searches every time.
    if best.is_none() {
        if let Some(english) = &english {
            if !english.eq_ignore_ascii_case(&primary_query) {
                cards = search_site(app, &mirrors, english, a.as_ref()).await?;
                best = crate::matching::best_match(&[english], &to_candidates(&cards));
            }
        }
    }

    let Some(m) = best else { return Ok((LinkOutcome::NoMatch, None)) };
    let matched = cards[m.index].clone();
    let new_slug = slug_from_url(&matched.url);

    // Slug collision: the matched site series may already be tracked (e.g.
    // it's on the airing list). Merge onto that existing row instead of
    // touching slug/url at all — no extra scrape needed, the existing row
    // already has real episodes/genres from its own normal scrape.
    if let Some(existing_id) = {
        let db = state.db.lock().unwrap();
        db.find_series_id_by_slug(info.source_id, &new_slug, series_id).map_err(|e| e.to_string())?
    } {
        let db = state.db.lock().unwrap();
        db.merge_series_into(existing_id, series_id).map_err(|e| e.to_string())?;
        let episodes = db.list_series_episodes(existing_id).map_err(|e| e.to_string())?.len() as i64;
        let url = db
            .get_series_url(existing_id)
            .map_err(|e| e.to_string())?
            .unwrap_or(matched.url.clone());
        return Ok((LinkOutcome::Linked { url, episodes }, Some(existing_id)));
    }

    // No collision: fetch the matched series page once, reused for both the
    // episode list and the detail (genres/kind) parse.
    let scraped = fetch_html(app, &matched.url).await.map_err(|e| e.to_string())?;
    let episodes = a.parse_series(&scraped.html).map_err(|e| e.to_string())?;
    let detail = a.parse_series_detail(&scraped.html).map_err(|e| e.to_string())?;
    let kind = detail.kind.unwrap_or(matched.kind.clone());

    let db = state.db.lock().unwrap();
    db.relink_series(series_id, &new_slug, &matched.url, matched.poster_url.as_deref(), &kind)
        .map_err(|e| e.to_string())?;
    db.replace_series_genres(series_id, &detail.genres).map_err(|e| e.to_string())?;
    let episode_count = episodes.len() as i64;
    for mut e in episodes {
        e.series_id = series_id;
        db.insert_episode(&e).map_err(|e| e.to_string())?;
    }
    if info.watched_externally {
        db.mark_all_episodes_seen(series_id).map_err(|e| e.to_string())?;
    }

    Ok((LinkOutcome::Linked { url: matched.url, episodes: episode_count }, Some(series_id)))
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
    fn push_history_reinserts_a_repeated_id_at_the_front() {
        // Re-deciding the same series id moves it to the front; the deque may
        // then hold it twice, but list_swipe_history/undo tolerate that
        // (rows self-heal / retain removes all copies).
        let mut h = VecDeque::new();
        push_history(&mut h, 10);
        push_history(&mut h, 20);
        push_history(&mut h, 10);
        assert_eq!(h.front().copied(), Some(10));
    }

    #[test]
    fn filter_candidate_genres_drops_baseline_and_user_bans_case_insensitively() {
        let all = vec![
            "Action".to_string(),
            "Hentai".to_string(),  // always-on baseline
            "ecchi".to_string(),   // baseline, different case
            "Horror".to_string(),  // user-banned below
            "Drama".to_string(),
        ];
        let out = filter_candidate_genres(all, &["horror".to_string()]);
        assert_eq!(out, vec!["Action".to_string(), "Drama".to_string()]);
    }

    #[test]
    fn decision_for_history_row_derives_from_live_flags() {
        let seen = crate::db::SwipeHistoryRow {
            title: "S".into(), poster_url: None,
            backlog_status: None, watched_externally: true,
        };
        assert_eq!(decision_for_history_row(&seen), "seen");
        let want = crate::db::SwipeHistoryRow {
            title: "W".into(), poster_url: None,
            backlog_status: Some("want".into()), watched_externally: false,
        };
        assert_eq!(decision_for_history_row(&want), "want");
        let disc = crate::db::SwipeHistoryRow {
            title: "D".into(), poster_url: None,
            backlog_status: Some("discarded".into()), watched_externally: false,
        };
        assert_eq!(decision_for_history_row(&disc), "discard");
        let none = crate::db::SwipeHistoryRow {
            title: "N".into(), poster_url: None,
            backlog_status: None, watched_externally: false,
        };
        assert_eq!(decision_for_history_row(&none), "none");
    }

    #[test]
    fn load_mirrors_migrates_legacy_global_key_into_animeytx_scoped_key_once() {
        let db = Db::open(":memory:").unwrap();
        db.set_setting(LEGACY_MIRRORS_KEY, "https://old-mirror.example\nhttps://old-mirror-2.example").unwrap();

        // First read: no site-scoped key yet, falls back to (and persists into) it.
        let mirrors = load_mirrors(&db, "animeytx").unwrap();
        assert_eq!(mirrors, vec!["https://old-mirror.example", "https://old-mirror-2.example"]);
        // The legacy key must survive untouched — it's the rollback path.
        assert_eq!(
            db.get_setting(LEGACY_MIRRORS_KEY).unwrap().as_deref(),
            Some("https://old-mirror.example\nhttps://old-mirror-2.example")
        );
        // And the migration wrote the site-scoped key forward.
        assert_eq!(
            db.get_setting("mirror_urls:animeytx").unwrap().as_deref(),
            Some("https://old-mirror.example\nhttps://old-mirror-2.example")
        );
    }

    #[test]
    fn load_mirrors_does_not_fall_back_to_legacy_key_for_a_non_animeytx_site() {
        // A brand-new site with no site-scoped mirrors yet must start empty,
        // never silently inherit AnimeYT's old global mirror list.
        let db = Db::open(":memory:").unwrap();
        db.set_setting(LEGACY_MIRRORS_KEY, "https://old-mirror.example").unwrap();
        assert_eq!(load_mirrors(&db, "tioanime").unwrap(), Vec::<String>::new());
    }

    #[test]
    fn list_sites_matches_the_adapter_registry() {
        let sites = list_sites();
        assert_eq!(sites.len(), adapter::all_sites().len());
        assert!(sites.iter().any(|s| s.id == "animeytx"));
        assert!(sites.iter().any(|s| s.id == "tioanime"));
        assert!(sites.iter().any(|s| s.id == "animeflv"));
    }

    #[test]
    fn switch_site_core_rejects_an_unknown_site() {
        let db = Db::open(":memory:").unwrap();
        let err = switch_site_core(&db, "not-a-real-site").unwrap_err();
        assert!(err.contains("desconocido"));
    }

    #[test]
    fn switch_site_core_seeds_default_mirrors_on_first_switch() {
        let db = Db::open(":memory:").unwrap();
        let (result, existing_source) = switch_site_core(&db, "tioanime").unwrap();
        assert!(result.is_first_time, "never scanned before -> first_time");
        assert_eq!(existing_source, None, "no sources row yet -> nothing to restore");
        assert_eq!(load_mirrors(&db, "tioanime").unwrap(), vec!["https://tioanime.com"]);
        assert_eq!(db.get_setting(ACTIVE_SITE_KEY).unwrap().as_deref(), Some("tioanime"));
    }

    #[test]
    fn switch_site_core_does_not_clobber_mirrors_the_user_already_configured() {
        let db = Db::open(":memory:").unwrap();
        save_mirrors(&db, "tioanime", &["https://custom-mirror.example".to_string()]).unwrap();
        let (_result, _) = switch_site_core(&db, "tioanime").unwrap();
        assert_eq!(load_mirrors(&db, "tioanime").unwrap(), vec!["https://custom-mirror.example"]);
    }

    #[test]
    fn switch_site_core_restores_an_existing_source_and_is_not_first_time() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("TioAnime", "https://tioanime.com", "tioanime").unwrap();
        let (result, existing_source) = switch_site_core(&db, "tioanime").unwrap();
        assert!(!result.is_first_time);
        assert_eq!(existing_source, Some(src));
    }

    /// Switching site A -> B -> A must not touch A's series rows at all —
    /// the core "library is per-site, nothing is deleted" guarantee. This
    /// only exercises the settings/sources bookkeeping switch_site_core
    /// owns; the `series` table itself is never written by it, which is the
    /// point (see the design spec's scope note).
    #[test]
    fn switch_site_core_round_trip_leaves_series_rows_untouched() {
        let db = Db::open(":memory:").unwrap();
        let src_a = db.upsert_source("AnimeYT", "https://wwv.animeytx.net", "animeytx").unwrap();
        let s = crate::models::Series {
            id: 0, slug: "a".into(), title: "A".into(), url: "u".into(), cover_url: None,
            is_airing: true, followed: true, next_episode_at: None, site_episode_count: None,
        };
        let sid = db.upsert_series(src_a, &s).unwrap();
        db.set_followed(sid, true).unwrap();

        switch_site_core(&db, "tioanime").unwrap();
        switch_site_core(&db, "animeytx").unwrap();

        let (count, followed): (i64, i64) = db
            .conn
            .query_row(
                "SELECT COUNT(*), SUM(followed) FROM series WHERE source_id=?1",
                [src_a],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(count, 1);
        assert_eq!(followed, 1);
    }

    fn insert_test_series(db: &Db, src: i64, slug: &str) -> i64 {
        let s = crate::models::Series {
            id: 0, slug: slug.into(), title: slug.into(), url: format!("u-{slug}"),
            cover_url: None, is_airing: false, followed: false, next_episode_at: None, site_episode_count: None,
        };
        db.upsert_series(src, &s).unwrap()
    }

    /// Watching -> None ("Dejar de seguir"): `followed` clears and no
    /// backlog/watched-externally signal is left behind.
    #[test]
    fn reclassify_series_core_watching_to_none_clears_followed() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
        let sid = insert_test_series(&db, src, "a");
        db.set_followed(sid, true).unwrap();

        reclassify_series_core(&db, sid, Classification::None).unwrap();

        assert!(!db.list_followed(src).unwrap().iter().any(|f| f.id == sid));
        assert_eq!(db.get_backlog_status(sid).unwrap(), None);
    }

    /// Watching -> Want ("Mover a Quiero ver"): unfollows and lands in the
    /// 'want' backlog in the same atomic step.
    #[test]
    fn reclassify_series_core_watching_to_want() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
        let sid = insert_test_series(&db, src, "a");
        db.set_followed(sid, true).unwrap();

        reclassify_series_core(&db, sid, Classification::Want).unwrap();

        assert!(!db.list_followed(src).unwrap().iter().any(|f| f.id == sid));
        assert_eq!(db.get_backlog_status(sid).unwrap().as_deref(), Some("want"));
    }

    /// Want -> None ("Quitar de la lista"): clears backlog_status without
    /// discarding (distinct from Want -> Discarded).
    #[test]
    fn reclassify_series_core_want_to_none() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
        let sid = insert_test_series(&db, src, "a");
        db.set_backlog_status(sid, Some("want")).unwrap();

        reclassify_series_core(&db, sid, Classification::None).unwrap();

        assert_eq!(db.get_backlog_status(sid).unwrap(), None);
    }

    /// WatchedExternally -> None ("Ya no la he visto"): clears the flag so
    /// the row is unclassified again (reappears in future decks).
    #[test]
    fn reclassify_series_core_watched_externally_to_none() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
        let sid = insert_test_series(&db, src, "a");
        db.set_watched_externally(sid, true).unwrap();

        reclassify_series_core(&db, sid, Classification::None).unwrap();

        assert!(db.list_watched_externally(src).unwrap().iter().all(|s| s.id != sid));
    }

    /// WatchedExternally -> Want ("Mover a Quiero ver" from Ya vistas).
    #[test]
    fn reclassify_series_core_watched_externally_to_want() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
        let sid = insert_test_series(&db, src, "a");
        db.set_watched_externally(sid, true).unwrap();

        reclassify_series_core(&db, sid, Classification::Want).unwrap();

        assert!(db.list_watched_externally(src).unwrap().iter().all(|s| s.id != sid));
        assert_eq!(db.get_backlog_status(sid).unwrap().as_deref(), Some("want"));
    }

    /// Any -> WatchedExternally applies the flag and clears whatever else
    /// was set (defensive: reclassify always fully clears first).
    #[test]
    fn reclassify_series_core_want_to_watched_externally() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
        let sid = insert_test_series(&db, src, "a");
        db.set_backlog_status(sid, Some("want")).unwrap();

        reclassify_series_core(&db, sid, Classification::WatchedExternally).unwrap();

        assert_eq!(db.get_backlog_status(sid).unwrap(), None);
        assert!(db.list_watched_externally(src).unwrap().iter().any(|s| s.id == sid));
    }

    /// Reclassify is a pure local state move: it must never touch
    /// `episodes` rows (seen/unseen), even when unfollowing a series with
    /// watched history.
    #[test]
    fn reclassify_series_core_never_touches_episodes() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
        let sid = insert_test_series(&db, src, "a");
        db.set_followed(sid, true).unwrap();
        db.insert_episode(&crate::models::Episode {
            id: 0, series_id: sid, number: "1".into(), title: None,
            url: "e1".into(), released_at: None, seen: false,
        }).unwrap();
        db.set_seen_cascade(sid, "1", true).unwrap();

        reclassify_series_core(&db, sid, Classification::Want).unwrap();

        let eps = db.list_series_episodes(sid).unwrap();
        assert_eq!(eps.len(), 1);
        assert!(eps[0].seen, "episode seen state must survive a reclassify");
    }

    #[test]
    fn mirrors_are_isolated_per_site() {
        let db = Db::open(":memory:").unwrap();
        save_mirrors(&db, "animeytx", &["https://a.example".to_string()]).unwrap();
        save_mirrors(&db, "tioanime", &["https://b.example".to_string()]).unwrap();
        assert_eq!(load_mirrors(&db, "animeytx").unwrap(), vec!["https://a.example"]);
        assert_eq!(load_mirrors(&db, "tioanime").unwrap(), vec!["https://b.example"]);
    }

    const NOW: i64 = 1_783_400_000;
    const DAY: i64 = 86_400;

    /// Baseline "quiet week" listing series: next episode in the future and
    /// the site's badge shows the *upcoming* episode's number (db+1, the
    /// live-verified up-to-date pattern) — the case that must skip for the
    /// big win to exist.
    #[test]
    fn skip_when_next_episode_in_future_and_no_count_conflict() {
        assert!(!should_fetch_series(
            false, true, true, Some(NOW + DAY), None, 5, None, NOW
        ));
        // .sb = db+1: up to date under the verified next-episode-number
        // semantics (8/8 provably-current live series showed exactly this).
        assert!(!should_fetch_series(
            false, true, true, Some(NOW + DAY), Some(6), 5, None, NOW
        ));
        // .sb = db: db numbering equal/ahead of the badge — also nothing new.
        assert!(!should_fetch_series(
            false, true, true, Some(NOW + DAY), Some(5), 5, None, NOW
        ));
    }

    #[test]
    fn fetch_when_next_episode_in_past_and_count_unknown() {
        assert!(should_fetch_series(
            false, true, true, Some(NOW - 3600), None, 5, None, NOW
        ));
    }

    /// A countdown sitting in the past means the episode aired and the card
    /// hasn't rolled over — always fetch, regardless of what the badge says
    /// (the badge may not have rolled yet either).
    #[test]
    fn fetch_when_countdown_in_past_even_if_badge_looks_current() {
        assert!(should_fetch_series(
            false, true, true, Some(NOW - 3600), Some(6), 5, None, NOW
        ));
        assert!(should_fetch_series(
            false, true, true, Some(NOW - 3600), Some(5), 5, None, NOW
        ));
    }

    /// .sb greater than db+1 means the site has posted episodes we don't
    /// have (badge = next episode's number, so badge > db+1 ⇒ posted > db).
    /// This must win even over a future countdown: the site rolls the
    /// countdown to next week when it posts, so "future next_episode_at"
    /// alone would skip a freshly-posted episode forever.
    #[test]
    fn fetch_when_badge_says_site_is_ahead_even_with_future_countdown() {
        assert!(should_fetch_series(
            false, true, true, Some(NOW + DAY), Some(7), 5, None, NOW
        ));
    }

    #[test]
    fn fetch_when_no_signal_at_all_on_listing() {
        // On the listing but no countdown span and a non-numeric count
        // ("??") — no basis to skip.
        assert!(should_fetch_series(false, true, true, None, None, 5, None, NOW));
    }

    /// Followed series absent from the airing listing: fetch at most once
    /// per OFF_LISTING_RECHECK interval (24h — live verification showed the
    /// listing only carries ~77 series and genuinely-airing followed shows
    /// can be absent from it, so "absent = finished" is false and a 7-day
    /// interval would delay real episodes up to a week).
    #[test]
    fn off_listing_series_fetches_only_when_recheck_interval_elapsed() {
        // Never checked → fetch.
        assert!(should_fetch_series(false, true, false, None, None, 5, None, NOW));
        // Checked 1 hour ago → skip.
        assert!(!should_fetch_series(
            false, true, false, None, None, 5, Some(3600), NOW
        ));
        // Checked 25 hours ago → fetch again.
        assert!(should_fetch_series(
            false, true, false, None, None, 5, Some(25 * 3600), NOW
        ));
    }

    #[test]
    fn force_always_fetches() {
        // force=true overrides every skip rule, including the strongest ones.
        assert!(should_fetch_series(
            true, true, true, Some(NOW + DAY), Some(5), 5, Some(0), NOW
        ));
        assert!(should_fetch_series(true, true, false, None, None, 5, Some(0), NOW));
    }

    #[test]
    fn listing_scan_failure_falls_back_to_fetching_everything() {
        // If the airing-listing scan failed there is no fresh metadata; the
        // refresh must behave exactly like the pre-skip-logic code and fetch
        // every followed series rather than trusting stale skip signals.
        assert!(should_fetch_series(
            false, false, true, Some(NOW + DAY), Some(5), 5, Some(0), NOW
        ));
    }
}
