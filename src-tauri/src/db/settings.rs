use super::*;

impl Db {
    pub fn get_setting(&self, key: &str) -> Result<Option<String>> {
        let v = self
            .conn
            .query_row("SELECT value FROM settings WHERE key=?1", [key], |r| {
                r.get::<_, String>(0)
            })
            .ok();
        Ok(v)
    }

    pub fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO settings(key, value) VALUES(?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            (key, value),
        )?;
        Ok(())
    }

    pub fn delete_setting(&self, key: &str) -> Result<()> {
        self.conn.execute("DELETE FROM settings WHERE key=?1", [key])?;
        Ok(())
    }

    /// Global (not per-site) user-configured genre ban list for the
    /// Descubrir catalog deck — un-prefixed `banned_genres` settings key,
    /// newline-joined like the per-site mirror list (see
    /// `commands::{load_mirrors, save_mirrors}`), but global because taste
    /// bans are a user preference, not tied to whichever site happens to be
    /// active. No hardcoded baseline exclusion — purely user-driven.
    pub fn get_banned_genres(&self) -> Result<Vec<String>> {
        Ok(self
            .get_setting("banned_genres")?
            .map(|raw| raw.lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect())
            .unwrap_or_default())
    }

    pub fn set_banned_genres(&self, genres: &[String]) -> Result<()> {
        self.set_setting("banned_genres", &genres.join("\n"))
    }

    /// Global user-configured format ("tipo") ban list for the Descubrir
    /// catalog deck — un-prefixed `banned_formats` settings key, same
    /// newline-joined shape as `get_banned_genres`. Values are expected to
    /// be a subset of `['TV','MOVIE','OVA','ONA','SPECIAL']` (the deck's
    /// default whitelist), but this getter doesn't validate that — the
    /// consuming query (`random_catalog_anime_in_genre`) just filters the
    /// whitelist against whatever's stored, so an unrecognized value is
    /// harmlessly a no-op.
    pub fn get_banned_formats(&self) -> Result<Vec<String>> {
        Ok(self
            .get_setting("banned_formats")?
            .map(|raw| raw.lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect())
            .unwrap_or_default())
    }

    pub fn set_banned_formats(&self, formats: &[String]) -> Result<()> {
        self.set_setting("banned_formats", &formats.join("\n"))
    }

    /// Global toggle: exclude `NOT_YET_RELEASED` titles from the Descubrir
    /// catalog deck (see `db::catalog::random_catalog_anime_in_genre`'s
    /// `hide_upcoming` param). Absent key (never set) defaults to `false` —
    /// the deck's pre-existing behavior.
    pub fn get_hide_upcoming_releases(&self) -> Result<bool> {
        Ok(self.get_setting("hide_upcoming_releases")?.as_deref() == Some("true"))
    }

    pub fn set_hide_upcoming_releases(&self, hide: bool) -> Result<()> {
        self.set_setting("hide_upcoming_releases", if hide { "true" } else { "false" })
    }

    /// Global lower bound (Unix seconds) on `anilist_catalog.start_date` for
    /// the Descubrir catalog deck. Absent key = `None` = unbounded ("since
    /// forever") — the deck's pre-existing behavior, no default substitution.
    pub fn get_deck_min_start_date(&self) -> Result<Option<i64>> {
        Ok(self.get_setting("deck_min_start_date")?.and_then(|v| v.parse().ok()))
    }

    pub fn set_deck_min_start_date(&self, value: Option<i64>) -> Result<()> {
        match value {
            Some(v) => self.set_setting("deck_min_start_date", &v.to_string()),
            None => self.delete_setting("deck_min_start_date"),
        }
    }

    /// Global upper bound (Unix seconds) on `anilist_catalog.start_date`.
    /// Absent key = `None` — the caller (`commands::discover_catalog_card`)
    /// substitutes "today" as a moving default; this getter itself never
    /// does, so it stays a plain, testable settings round-trip.
    pub fn get_deck_max_start_date(&self) -> Result<Option<i64>> {
        Ok(self.get_setting("deck_max_start_date")?.and_then(|v| v.parse().ok()))
    }

    pub fn set_deck_max_start_date(&self, value: Option<i64>) -> Result<()> {
        match value {
            Some(v) => self.set_setting("deck_max_start_date", &v.to_string()),
            None => self.delete_setting("deck_max_start_date"),
        }
    }

    /// User-preferred target monitor for opening episodes.
    /// `None` or `"default"` falls back to the current window's monitor or primary monitor.
    pub fn get_player_monitor(&self) -> Result<Option<String>> {
        self.get_setting("player_monitor")
    }

    pub fn set_player_monitor(&self, value: Option<&str>) -> Result<()> {
        match value {
            Some(v) if !v.trim().is_empty() => self.set_setting("player_monitor", v.trim()),
            _ => self.delete_setting("player_monitor"),
        }
    }

    /// User preference: open episode viewing windows in full screen mode.
    /// Defaults to `false` (windowed 1280x800).
    pub fn get_player_fullscreen(&self) -> Result<bool> {
        Ok(self.get_setting("player_fullscreen")?.as_deref() == Some("true"))
    }

    pub fn set_player_fullscreen(&self, fullscreen: bool) -> Result<()> {
        self.set_setting("player_fullscreen", if fullscreen { "true" } else { "false" })
    }


    /// Record "this series' episode list was actually fetched just now" —
    /// gates the FINISHED_RECHECK interval for followed series absent from
    /// the airing listing (see `commands::should_fetch_series`).
    pub fn set_last_checked_at(&self, series_id: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE series SET last_checked_at=datetime('now') WHERE id=?1",
            [series_id],
        )?;
        Ok(())
    }

    /// Seconds since the last recorded episode-list fetch, or `None` if the
    /// series has never had one recorded. Computed in SQL so the stored ISO
    /// 8601 text never needs parsing in Rust.
    // Kept as a public single-series lookup even though `refresh()` now uses
    // the batched `last_checked_ages_for_source` instead (see its doc
    // comment) — a real, useful primitive on its own, exercised directly by
    // the tests below.
    #[allow(dead_code)]
    pub fn last_checked_age_secs(&self, series_id: i64) -> Result<Option<i64>> {
        Ok(self.conn.query_row(
            "SELECT strftime('%s','now') - strftime('%s', last_checked_at)
             FROM series WHERE id=?1",
            [series_id],
            |r| r.get(0),
        )?)
    }

    /// Batched form of `last_checked_age_secs` for every followed series of
    /// `source_id` — see `Db::max_episode_numbers_for_source`'s doc comment
    /// for why `refresh()`'s per-series query pair was worth batching. A
    /// series missing from the map has no rows in `series` for this source
    /// at all (shouldn't happen for an id drawn from `list_followed`, but
    /// callers should still treat a missing key the same as `None` rather
    /// than panicking).
    pub fn last_checked_ages_for_source(
        &self,
        source_id: i64,
    ) -> Result<std::collections::HashMap<i64, Option<i64>>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, strftime('%s','now') - strftime('%s', last_checked_at)
             FROM series WHERE source_id=?1 AND followed=1",
        )?;
        let rows = stmt
            .query_map([source_id], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Option<i64>>(1)?)))?
            .collect::<rusqlite::Result<std::collections::HashMap<i64, Option<i64>>>>()?;
        Ok(rows)
    }

    /// Record that all sites have been searched for this canonical show and none had episodes.
    pub fn record_episode_fallback_miss(&self, canon_key: &str, now: i64) -> Result<()> {
        self.set_setting(&format!("episode_fallback_miss:{}", canon_key), &now.to_string())
    }

    /// Check if this show was checked recently (within 3 days) and had no episodes anywhere.
    pub fn has_recent_episode_fallback_miss(&self, canon_key: &str, now: i64) -> Result<bool> {
        if let Some(v) = self.get_setting(&format!("episode_fallback_miss:{}", canon_key))? {
            if let Ok(time) = v.parse::<i64>() {
                if now - time < 3 * 24 * 3600 {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }

    /// Record that a specific site failed to yield episodes for this show.
    /// Cool-down between two attempts at the same show by the background episode fallback.
    const EPISODE_FALLBACK_ATTEMPT_COOLDOWN_SECS: i64 = 30 * 60;

    pub fn record_episode_fallback_attempt(&self, canon_key: &str, now: i64) -> Result<()> {
        self.set_setting(&format!("episode_fallback_attempt:{}", canon_key), &now.to_string())
    }

    pub fn recently_attempted_episode_fallback(&self, canon_key: &str, now: i64) -> Result<bool> {
        if let Some(v) = self.get_setting(&format!("episode_fallback_attempt:{}", canon_key))? {
            if let Ok(t) = v.parse::<i64>() {
                return Ok(now - t < Self::EPISODE_FALLBACK_ATTEMPT_COOLDOWN_SECS);
            }
        }
        Ok(false)
    }

    pub fn record_episode_fallback_site_miss(&self, canon_key: &str, site_id: &str, now: i64) -> Result<()> {
        self.set_setting(&format!("episode_fallback_site_miss:{}:{}", site_id, canon_key), &now.to_string())
    }

    /// Check if a specific site failed recently (within 3 days).
    pub fn has_recent_episode_fallback_site_miss(&self, canon_key: &str, site_id: &str, now: i64) -> Result<bool> {
        if let Some(v) = self.get_setting(&format!("episode_fallback_site_miss:{}:{}", site_id, canon_key))? {
            if let Ok(time) = v.parse::<i64>() {
                if now - time < 3 * 24 * 3600 {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }

    pub fn get_site_priority(&self, known: &[&str]) -> Result<Vec<String>> {
        if let Some(raw) = self.get_setting("site_priority")? {
            if let Ok(stored) = serde_json::from_str::<Vec<String>>(&raw) {
                let mut result = Vec::new();
                let mut seen = std::collections::HashSet::new();
                for id in stored {
                    if known.contains(&id.as_str()) && seen.insert(id.clone()) {
                        result.push(id);
                    }
                }
                for &id in known {
                    if seen.insert(id.to_string()) {
                        result.push(id.to_string());
                    }
                }
                return Ok(result);
            }
        }
        Ok(known.iter().map(|&s| s.to_string()).collect())
    }

    pub fn set_site_priority(&self, order: &[String]) -> Result<()> {
        let json = serde_json::to_string(order)?;
        self.set_setting("site_priority", &json)
    }

    /// Discover niche bias in `[-1.0, 1.0]`, where -1.0 is strongly mainstream,
    /// +1.0 is strongly niche/cult, and 0.0 is neutral (default).
    pub fn get_discover_niche(&self) -> Result<f64> {
        let val = self
            .get_setting("discover_niche_bias")?
            .and_then(|v| v.parse::<f64>().ok())
            .filter(|v| !v.is_nan())
            .unwrap_or(0.0)
            .clamp(-1.0, 1.0);
        Ok(val)
    }

    pub fn set_discover_niche(&self, v: f64) -> Result<()> {
        let clamped = if v.is_nan() { 0.0 } else { v.clamp(-1.0, 1.0) };
        self.set_setting("discover_niche_bias", &format!("{:.2}", clamped))
    }

    /// Quality preference for offline downloads: "best" (default), "1080", "720", "480".
    pub fn get_download_quality(&self) -> Result<String> {
        Ok(self.get_setting("download_quality")?.unwrap_or_else(|| "best".to_string()))
    }

    pub fn set_download_quality(&self, quality: &str) -> Result<()> {
        let val = match quality {
            "1080" => "1080",
            "720" => "720",
            "480" => "480",
            _ => "best",
        };
        self.set_setting("download_quality", val)
    }

    /// Whether downloaded episodes are automatically deleted when marked as seen.
    /// Default: true (preserves existing behavior).
    pub fn get_auto_delete_seen_downloads(&self) -> Result<bool> {
        Ok(self
            .get_setting("auto_delete_seen_downloads")?
            .map(|v| v == "true")
            .unwrap_or(true))
    }

    pub fn set_auto_delete_seen_downloads(&self, enabled: bool) -> Result<()> {
        self.set_setting(
            "auto_delete_seen_downloads",
            if enabled { "true" } else { "false" },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_support::*;

    #[test]
    fn episode_fallback_miss_memory_expires_after_3_days() {
        let db = Db::open(":memory:").unwrap();
        let now = 10_000_000;

        // Not checked yet
        assert!(!db.has_recent_episode_fallback_miss("al:21", now).unwrap());

        // Record a miss
        db.record_episode_fallback_miss("al:21", now).unwrap();
        assert!(db.has_recent_episode_fallback_miss("al:21", now).unwrap());
        assert!(db.has_recent_episode_fallback_miss("al:21", now + 2 * 24 * 3600).unwrap()); // 2 days later is recent
        assert!(!db.has_recent_episode_fallback_miss("al:21", now + 4 * 24 * 3600).unwrap()); // 4 days later is not

        // Site miss
        assert!(!db.has_recent_episode_fallback_site_miss("al:21", "tioanime", now).unwrap());
        db.record_episode_fallback_site_miss("al:21", "tioanime", now).unwrap();
        assert!(db.has_recent_episode_fallback_site_miss("al:21", "tioanime", now).unwrap());
        assert!(db.has_recent_episode_fallback_site_miss("al:21", "tioanime", now + 2 * 24 * 3600).unwrap());
        assert!(!db.has_recent_episode_fallback_site_miss("al:21", "tioanime", now + 4 * 24 * 3600).unwrap());
        assert!(!db.has_recent_episode_fallback_site_miss("al:21", "jkanime", now).unwrap(), "different site");
    }

    #[test]
    fn last_checked_age_none_until_set_then_small() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
        let sid = db.upsert_series(src, &mk_airing("a", "A", None)).unwrap();
        assert_eq!(db.last_checked_age_secs(sid).unwrap(), None);
        db.set_last_checked_at(sid).unwrap();
        let age = db.last_checked_age_secs(sid).unwrap().expect("age after set");
        assert!((0..60).contains(&age), "freshly-set age should be ~0s, got {age}");
    }

    #[test]
    fn last_checked_ages_for_source_matches_the_per_series_version_and_scopes_correctly() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
        let other_src = db.upsert_source("TioAnime", "t", "tioanime").unwrap();

        let sid_checked = db.upsert_series(src, &mk_airing("a", "A", None)).unwrap();
        db.set_followed(sid_checked, true).unwrap();
        db.set_last_checked_at(sid_checked).unwrap();

        let sid_never = db.upsert_series(src, &mk_airing("b", "B", None)).unwrap();
        db.set_followed(sid_never, true).unwrap();

        let sid_unfollowed = db.upsert_series(src, &mk_airing("c", "C", None)).unwrap();
        db.set_last_checked_at(sid_unfollowed).unwrap();

        let sid_other = db.upsert_series(other_src, &mk_airing("d", "D", None)).unwrap();
        db.set_followed(sid_other, true).unwrap();

        let map = db.last_checked_ages_for_source(src).unwrap();
        assert!(map.get(&sid_checked).copied().flatten().is_some());
        assert_eq!(map.get(&sid_never).copied().flatten(), None);
        assert!(!map.contains_key(&sid_unfollowed), "not followed -> excluded");
        assert!(!map.contains_key(&sid_other), "different source -> excluded");
    }

    #[test]
    fn banned_genres_and_formats_round_trip_through_settings() {
        let db = Db::open(":memory:").unwrap();
        assert_eq!(db.get_banned_genres().unwrap(), Vec::<String>::new());
        assert_eq!(db.get_banned_formats().unwrap(), Vec::<String>::new());

        db.set_banned_genres(&["Horror".to_string(), "Mecha".to_string()]).unwrap();
        db.set_banned_formats(&["OVA".to_string()]).unwrap();

        assert_eq!(db.get_banned_genres().unwrap(), vec!["Horror".to_string(), "Mecha".to_string()]);
        assert_eq!(db.get_banned_formats().unwrap(), vec!["OVA".to_string()]);
    }

    #[test]
    fn hide_upcoming_releases_defaults_false_and_round_trips_through_settings() {
        let db = Db::open(":memory:").unwrap();
        assert!(!db.get_hide_upcoming_releases().unwrap());

        db.set_hide_upcoming_releases(true).unwrap();
        assert!(db.get_hide_upcoming_releases().unwrap());

        db.set_hide_upcoming_releases(false).unwrap();
        assert!(!db.get_hide_upcoming_releases().unwrap());
    }

    #[test]
    fn deck_date_range_defaults_none_and_round_trips_through_settings() {
        let db = Db::open(":memory:").unwrap();
        assert_eq!(db.get_deck_min_start_date().unwrap(), None);
        assert_eq!(db.get_deck_max_start_date().unwrap(), None);

        db.set_deck_min_start_date(Some(1_000)).unwrap();
        db.set_deck_max_start_date(Some(2_000)).unwrap();
        assert_eq!(db.get_deck_min_start_date().unwrap(), Some(1_000));
        assert_eq!(db.get_deck_max_start_date().unwrap(), Some(2_000));

        // Setting None after a Some clears it back to None, not the string "None".
        db.set_deck_min_start_date(None).unwrap();
        db.set_deck_max_start_date(None).unwrap();
        assert_eq!(db.get_deck_min_start_date().unwrap(), None);
        assert_eq!(db.get_deck_max_start_date().unwrap(), None);
    }

    #[test]
    fn player_settings_round_trip() {
        let db = Db::open(":memory:").unwrap();
        assert_eq!(db.get_player_monitor().unwrap(), None);
        assert!(!db.get_player_fullscreen().unwrap());

        db.set_player_monitor(Some("DISPLAY2")).unwrap();
        db.set_player_fullscreen(true).unwrap();
        assert_eq!(db.get_player_monitor().unwrap().as_deref(), Some("DISPLAY2"));
        assert!(db.get_player_fullscreen().unwrap());

        db.set_player_monitor(None).unwrap();
        db.set_player_fullscreen(false).unwrap();
        assert_eq!(db.get_player_monitor().unwrap(), None);
        assert!(!db.get_player_fullscreen().unwrap());
    }

    #[test]
    fn site_priority_defaults_to_known_when_nothing_stored() {
        let db = Db::open(":memory:").unwrap();
        assert_eq!(db.get_site_priority(&["a", "b", "c"]).unwrap(), vec!["a", "b", "c"]);
    }

    #[test]
    fn site_priority_preserves_stored_and_appends_missing_known() {
        let db = Db::open(":memory:").unwrap();
        db.set_site_priority(&["c".to_string(), "a".to_string()]).unwrap();
        assert_eq!(db.get_site_priority(&["a", "b", "c"]).unwrap(), vec!["c", "a", "b"]);
    }

    #[test]
    fn site_priority_drops_unknown_and_duplicate_stored_ids() {
        let db = Db::open(":memory:").unwrap();
        db.set_setting("site_priority", r#"["x","c","c","a"]"#).unwrap();
        assert_eq!(db.get_site_priority(&["a", "b", "c"]).unwrap(), vec!["c", "a", "b"]);
    }

    #[test]
    fn site_priority_round_trip() {
        let db = Db::open(":memory:").unwrap();
        let order = vec!["b".to_string(), "c".to_string(), "a".to_string()];
        db.set_site_priority(&order).unwrap();
        assert_eq!(db.get_site_priority(&["a", "b", "c"]).unwrap(), order);
        assert_eq!(db.get_setting("site_priority").unwrap().as_deref(), Some(r#"["b","c","a"]"#));
    }

    #[test]
    fn discover_niche_round_trip_and_clamping() {
        let db = Db::open(":memory:").unwrap();
        // unset -> 0.0
        assert_eq!(db.get_discover_niche().unwrap(), 0.0);

        // set 0.5 -> 0.5
        db.set_discover_niche(0.5).unwrap();
        assert!((db.get_discover_niche().unwrap() - 0.5).abs() < 1e-6);

        // set 7.0 -> stored/read as 1.0
        db.set_discover_niche(7.0).unwrap();
        assert_eq!(db.get_discover_niche().unwrap(), 1.0);

        // set -3.0 -> -1.0
        db.set_discover_niche(-3.0).unwrap();
        assert_eq!(db.get_discover_niche().unwrap(), -1.0);

        // garbage string stored under the key -> 0.0
        db.set_setting("discover_niche_bias", "not-a-number").unwrap();
        assert_eq!(db.get_discover_niche().unwrap(), 0.0);
    }

    #[test]
    fn download_quality_defaults_and_roundtrips() {
        let db = Db::open(":memory:").unwrap();
        // Default is "best"
        assert_eq!(db.get_download_quality().unwrap(), "best");

        db.set_download_quality("720").unwrap();
        assert_eq!(db.get_download_quality().unwrap(), "720");

        db.set_download_quality("1080").unwrap();
        assert_eq!(db.get_download_quality().unwrap(), "1080");

        db.set_download_quality("480").unwrap();
        assert_eq!(db.get_download_quality().unwrap(), "480");

        // Invalid value normalizes to "best"
        db.set_download_quality("invalid_quality").unwrap();
        assert_eq!(db.get_download_quality().unwrap(), "best");
    }

    #[test]
    fn auto_delete_seen_downloads_defaults_and_roundtrips() {
        let db = Db::open(":memory:").unwrap();
        // Default is true
        assert!(db.get_auto_delete_seen_downloads().unwrap());

        db.set_auto_delete_seen_downloads(false).unwrap();
        assert!(!db.get_auto_delete_seen_downloads().unwrap());

        db.set_auto_delete_seen_downloads(true).unwrap();
        assert!(db.get_auto_delete_seen_downloads().unwrap());
    }
}

