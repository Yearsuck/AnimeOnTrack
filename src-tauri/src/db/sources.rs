use super::*;

impl Db {
    /// Insert or update a source row, tagged with its stable `site_id`
    /// (`adapter::SiteInfo::id`). Conflict target stays `base_url` (matching
    /// the pre-existing scheme: a mirror fallback resolving to a different
    /// working URL creates/reuses a row keyed by that URL, same as before
    /// site_id existed) — `site_id` is written on every call so an existing
    /// row's tag stays correct even if it predates this column.
    pub fn upsert_source(&self, name: &str, base_url: &str, site_id: &str) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO sources(name, base_url, site_id) VALUES(?1, ?2, ?3)
             ON CONFLICT(base_url) DO UPDATE SET name=excluded.name, site_id=excluded.site_id",
            (name, base_url, site_id),
        )?;
        let id: i64 = self.conn.query_row(
            "SELECT id FROM sources WHERE base_url=?1",
            [base_url],
            |r| r.get(0),
        )?;
        Ok(id)
    }

    pub fn get_source_base_url(&self, source_id: i64) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row("SELECT base_url FROM sources WHERE id=?1", [source_id], |r| {
                r.get::<_, String>(0)
            })
            .ok())
    }

    /// The most recently-upserted source row tagged with `site_id`, or
    /// `None` if that site has never been scanned yet in this install. Used
    /// on app startup (restore the active source) and by the Settings site
    /// switcher (has this site already got a row to reuse?).
    pub fn get_source_id_for_site(&self, site_id: &str) -> Result<Option<i64>> {
        Ok(self
            .conn
            .query_row(
                "SELECT id FROM sources WHERE site_id=?1 ORDER BY id DESC LIMIT 1",
                [site_id],
                |r| r.get(0),
            )
            .ok())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    

    #[test]
    fn upsert_source_and_series_then_follow() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "https://wwv.animeytx.net", "animeytx").unwrap();

        let s = crate::models::Series {
            id: 0,
            slug: "baki-dou".into(),
            title: "Baki-dou".into(),
            url: "https://wwv.animeytx.net/tv/baki-dou/".into(),
            cover_url: None,
            is_airing: true,
            followed: false, next_episode_at: None, site_episode_count: None,
        };
        let sid = db.upsert_series(src, &s).unwrap();
        // upsert again with new title => same row updated, not duplicated
        let s2 = crate::models::Series { title: "Baki-dou 2".into(), ..s.clone() };
        let sid2 = db.upsert_series(src, &s2).unwrap();
        assert_eq!(sid, sid2);

        db.set_followed(sid, true).unwrap();
        let airing = db.list_airing(src).unwrap();
        assert_eq!(airing.len(), 1);
        assert!(airing[0].followed);
        assert_eq!(airing[0].title, "Baki-dou 2");
    }

    #[test]
    fn sources_site_id_backfilled_to_animeytx_on_reopen() {
        let path = std::env::temp_dir().join(format!("aot_site_id_migration_test_{}.sqlite", std::process::id()));
        let path_str = path.to_str().unwrap();
        let _ = std::fs::remove_file(&path);
        {
            let db = Db::open(path_str).unwrap();
            db.upsert_source("AnimeYT", "https://wwv.animeytx.net", "animeytx").unwrap();
            // Simulate a pre-migration row exactly as the design spec describes
            // ("there is exactly one, verify via SELECT * before writing the
            // migration"): site_id present in the schema but NULL on the row.
            db.conn.execute("UPDATE sources SET site_id = NULL", []).unwrap();
        }
        // Re-opening re-runs init_schema, which must backfill NULL site_id
        // rows to "animeytx" rather than leave them NULL.
        let db = Db::open(path_str).unwrap();
        let site_id: String = db
            .conn
            .query_row("SELECT site_id FROM sources WHERE base_url='https://wwv.animeytx.net'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(site_id, "animeytx");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn get_source_id_for_site_returns_none_when_never_scanned() {
        let db = Db::open(":memory:").unwrap();
        assert_eq!(db.get_source_id_for_site("tioanime").unwrap(), None);
    }

    #[test]
    fn get_source_id_for_site_finds_the_tagged_row_and_ignores_other_sites() {
        let db = Db::open(":memory:").unwrap();
        let animeytx_id = db.upsert_source("AnimeYT", "https://a.example", "animeytx").unwrap();
        let tioanime_id = db.upsert_source("TioAnime", "https://b.example", "tioanime").unwrap();
        assert_eq!(db.get_source_id_for_site("animeytx").unwrap(), Some(animeytx_id));
        assert_eq!(db.get_source_id_for_site("tioanime").unwrap(), Some(tioanime_id));
        assert_ne!(animeytx_id, tioanime_id);
    }
}
