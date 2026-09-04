use super::*;
use serde::Serialize;

/// Prefix for the per-site mirror-list settings key (`mirror_urls:{site_id}`).
/// The old global `mirror_urls` key (no site suffix) is left in the DB
/// untouched as a rollback path — see `load_mirrors`'s one-time migration.
const MIRRORS_KEY_PREFIX: &str = "mirror_urls";
/// Legacy pre-multi-site global mirrors key. Migrated once into
/// `mirror_urls:animeytx` by `load_mirrors` the first time that site-scoped
/// key is read and found empty.
const LEGACY_MIRRORS_KEY: &str = "mirror_urls";
/// Settings key holding the stable slug of the currently-active site
/// (`adapter::SiteInfo::id`). Absent on installs that predate multi-site
/// support — those default to `"animeytx"`, the only site that ever existed
/// before this key did.
const ACTIVE_SITE_KEY: &str = "active_site_id";
const DEFAULT_SITE_ID: &str = "animeytx";

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

#[derive(Serialize, Debug)]
pub struct SiteSwitchResult {
    pub site: SiteSummary,
    /// `true` when this site has never been scanned before in this install
    /// (no `sources` row tagged with it yet) — the caller still needs to
    /// call `scan_airing` either way, but this lets the UI say "first scan
    /// of {site}…" vs "back to {site}" if it wants to.
    pub is_first_time: bool,
}

/// Load the mirror list for `site_id`. One-time migration: if the site-scoped
/// key has never been written and this is the original site (`"animeytx"`),
/// fall back to the pre-multi-site global `mirror_urls` key, and persist it
/// forward under the site-scoped key so future reads don't need this check —
/// the old global key is left in place either way (harmless, and a rollback
/// path), per the design spec.
pub fn load_mirrors(db: &Db, site_id: &str) -> Result<Vec<String>, String> {
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

pub fn save_mirrors(db: &Db, site_id: &str, mirrors: &[String]) -> Result<(), String> {
    let key = format!("{MIRRORS_KEY_PREFIX}:{site_id}");
    db.set_setting(&key, &mirrors.join("\n")).map_err(|e| e.to_string())
}

/// Pure-enough-to-test core of `set_active_site`: everything that reads/
/// writes the DB, returning the resulting `source_id` for the caller to
/// stash in `state.source_id`. Split out from the `#[tauri::command]` wrapper
/// so it's testable against a plain in-memory `Db` without a live `State`.
pub fn switch_site_core(db: &Db, site_id: &str) -> Result<(SiteSwitchResult, Option<i64>), String> {
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
///
/// Mirror URLs eventually reach `WebviewUrl::External` navigation (airing
/// scans, episode fetches, cover images) — a mirror pointing at `file://` or
/// an internal network address is rejected here rather than only at
/// navigation time, so Settings gives immediate feedback instead of a scan
/// silently refusing every fetch against it later.
#[tauri::command]
pub fn set_mirrors(state: State<'_, AppState>, urls: Vec<String>) -> Result<(), String> {
    let site_id = get_active_site_id(&state);
    let db = state.db.lock().unwrap();
    let mut cleaned: Vec<String> = urls
        .iter()
        .map(|u| normalize(u))
        .filter(|u| !u.is_empty())
        .collect();
    if let Some(bad) = cleaned.iter().find(|u| !is_safe_external_url(u)) {
        return Err(format!("mirror no permitido: {bad}"));
    }
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

/// Every site the Settings selector can offer, in `adapter::all_sites()`
/// order.
#[tauri::command]
pub fn list_sites() -> Vec<SiteSummary> {
    adapter::all_sites().iter().map(SiteSummary::from).collect()
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

#[cfg(test)]
mod tests {
    use super::*;

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


    #[test]
    fn mirrors_are_isolated_per_site() {
        let db = Db::open(":memory:").unwrap();
        save_mirrors(&db, "animeytx", &["https://a.example".to_string()]).unwrap();
        save_mirrors(&db, "tioanime", &["https://b.example".to_string()]).unwrap();
        assert_eq!(load_mirrors(&db, "animeytx").unwrap(), vec!["https://a.example"]);
        assert_eq!(load_mirrors(&db, "tioanime").unwrap(), vec!["https://b.example"]);
    }
}
