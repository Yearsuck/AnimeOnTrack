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
    pub fn last_checked_age_secs(&self, series_id: i64) -> Result<Option<i64>> {
        Ok(self.conn.query_row(
            "SELECT strftime('%s','now') - strftime('%s', last_checked_at)
             FROM series WHERE id=?1",
            [series_id],
            |r| r.get(0),
        )?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_support::*;

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
}
