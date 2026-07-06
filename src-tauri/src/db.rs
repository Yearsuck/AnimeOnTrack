use anyhow::Result;
use rusqlite::Connection;

pub struct Db {
    pub conn: Connection,
}

impl Db {
    /// Open a DB at `path` (":memory:" for tests) and ensure schema exists.
    pub fn open(path: &str) -> Result<Self> {
        let conn = Connection::open(path)?;
        let db = Db { conn };
        db.init_schema()?;
        Ok(db)
    }

    fn init_schema(&self) -> Result<()> {
        self.conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS sources (
                id INTEGER PRIMARY KEY,
                name TEXT NOT NULL,
                base_url TEXT NOT NULL UNIQUE
            );
            CREATE TABLE IF NOT EXISTS series (
                id INTEGER PRIMARY KEY,
                source_id INTEGER NOT NULL REFERENCES sources(id),
                slug TEXT NOT NULL,
                title TEXT NOT NULL,
                url TEXT NOT NULL,
                cover_url TEXT,
                is_airing INTEGER NOT NULL DEFAULT 1,
                followed INTEGER NOT NULL DEFAULT 0,
                UNIQUE(source_id, slug)
            );
            CREATE TABLE IF NOT EXISTS episodes (
                id INTEGER PRIMARY KEY,
                series_id INTEGER NOT NULL REFERENCES series(id),
                number TEXT NOT NULL,
                title TEXT,
                url TEXT NOT NULL,
                released_at TEXT,
                seen INTEGER NOT NULL DEFAULT 0,
                added_at TEXT NOT NULL DEFAULT (datetime('now')),
                UNIQUE(series_id, url)
            );
            CREATE TABLE IF NOT EXISTS settings (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );
            "#,
        )?;
        Ok(())
    }

    pub fn upsert_source(&self, name: &str, base_url: &str) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO sources(name, base_url) VALUES(?1, ?2)
             ON CONFLICT(base_url) DO UPDATE SET name=excluded.name",
            (name, base_url),
        )?;
        let id: i64 = self.conn.query_row(
            "SELECT id FROM sources WHERE base_url=?1",
            [base_url],
            |r| r.get(0),
        )?;
        Ok(id)
    }

    pub fn upsert_series(&self, source_id: i64, s: &crate::models::Series) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO series(source_id, slug, title, url, cover_url, is_airing)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(source_id, slug) DO UPDATE SET
                title=excluded.title, url=excluded.url,
                cover_url=excluded.cover_url, is_airing=excluded.is_airing",
            (
                source_id, &s.slug, &s.title, &s.url,
                &s.cover_url, s.is_airing as i64,
            ),
        )?;
        let id: i64 = self.conn.query_row(
            "SELECT id FROM series WHERE source_id=?1 AND slug=?2",
            (source_id, &s.slug),
            |r| r.get(0),
        )?;
        Ok(id)
    }

    pub fn set_followed(&self, series_id: i64, followed: bool) -> Result<()> {
        self.conn.execute(
            "UPDATE series SET followed=?1 WHERE id=?2",
            (followed as i64, series_id),
        )?;
        Ok(())
    }

    /// Update a series' canonical URL (used when a mirror fallback succeeds on
    /// a different host than the one currently stored).
    pub fn update_series_url(&self, series_id: i64, url: &str) -> Result<()> {
        self.conn
            .execute("UPDATE series SET url=?1 WHERE id=?2", (url, series_id))?;
        Ok(())
    }

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

    fn row_to_series(r: &rusqlite::Row) -> rusqlite::Result<crate::models::Series> {
        Ok(crate::models::Series {
            id: r.get("id")?,
            slug: r.get("slug")?,
            title: r.get("title")?,
            url: r.get("url")?,
            cover_url: r.get("cover_url")?,
            is_airing: r.get::<_, i64>("is_airing")? != 0,
            followed: r.get::<_, i64>("followed")? != 0,
        })
    }

    pub fn list_airing(&self, source_id: i64) -> Result<Vec<crate::models::Series>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, slug, title, url, cover_url, is_airing, followed
             FROM series WHERE source_id=?1 AND is_airing=1 ORDER BY title",
        )?;
        let rows = stmt
            .query_map([source_id], Self::row_to_series)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    pub fn list_followed(&self, source_id: i64) -> Result<Vec<crate::models::Series>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, slug, title, url, cover_url, is_airing, followed
             FROM series WHERE source_id=?1 AND followed=1 ORDER BY title",
        )?;
        let rows = stmt
            .query_map([source_id], Self::row_to_series)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    pub fn existing_episode_urls(&self, series_id: i64) -> Result<std::collections::HashSet<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT url FROM episodes WHERE series_id=?1")?;
        let urls = stmt
            .query_map([series_id], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<std::collections::HashSet<_>>>()?;
        Ok(urls)
    }

    /// Insert episode if its (series_id, url) is new. Returns the row id either way.
    pub fn insert_episode(&self, e: &crate::models::Episode) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO episodes(series_id, number, title, url, released_at, seen)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(series_id, url) DO NOTHING",
            (
                e.series_id, &e.number, &e.title, &e.url,
                &e.released_at, e.seen as i64,
            ),
        )?;
        let id: i64 = self.conn.query_row(
            "SELECT id FROM episodes WHERE series_id=?1 AND url=?2",
            (e.series_id, &e.url),
            |r| r.get(0),
        )?;
        Ok(id)
    }

    /// Set an episode's seen flag either way (lets the user un-mark).
    pub fn set_seen(&self, episode_id: i64, seen: bool) -> Result<()> {
        self.conn.execute(
            "UPDATE episodes SET seen=?1 WHERE id=?2",
            (seen as i64, episode_id),
        )?;
        Ok(())
    }

    /// All episodes of a series (seen or not), oldest number first — the
    /// progress view for "which episode am I on".
    pub fn list_series_episodes(&self, series_id: i64) -> Result<Vec<crate::models::Episode>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, series_id, number, title, url, released_at, seen
             FROM episodes WHERE series_id=?1
             ORDER BY CAST(number AS INTEGER) ASC, id ASC",
        )?;
        let rows = stmt
            .query_map([series_id], |r| {
                Ok(crate::models::Episode {
                    id: r.get(0)?,
                    series_id: r.get(1)?,
                    number: r.get(2)?,
                    title: r.get(3)?,
                    url: r.get(4)?,
                    released_at: r.get(5)?,
                    seen: r.get::<_, i64>(6)? != 0,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Unseen episodes of currently-followed series only — unfollowing a
    /// series must drop its episodes out of the pending count immediately.
    pub fn pending_count(&self) -> Result<i64> {
        let n: i64 = self.conn.query_row(
            "SELECT count(*) FROM episodes e JOIN series s ON s.id = e.series_id
             WHERE e.seen=0 AND s.followed=1",
            [],
            |r| r.get(0),
        )?;
        Ok(n)
    }

    /// Unseen episodes of currently-followed series, joined with their
    /// series, newest first.
    pub fn list_pending(&self) -> Result<Vec<(crate::models::Series, crate::models::Episode)>> {
        let mut stmt = self.conn.prepare(
            "SELECT s.id, s.slug, s.title, s.url, s.cover_url, s.is_airing, s.followed,
                    e.id, e.series_id, e.number, e.title, e.url, e.released_at, e.seen
             FROM episodes e JOIN series s ON s.id = e.series_id
             WHERE e.seen=0 AND s.followed=1
             ORDER BY s.title, e.added_at DESC",
        )?;
        let rows = stmt
            .query_map([], |r| {
                let series = crate::models::Series {
                    id: r.get(0)?,
                    slug: r.get(1)?,
                    title: r.get(2)?,
                    url: r.get(3)?,
                    cover_url: r.get(4)?,
                    is_airing: r.get::<_, i64>(5)? != 0,
                    followed: r.get::<_, i64>(6)? != 0,
                };
                let ep = crate::models::Episode {
                    id: r.get(7)?,
                    series_id: r.get(8)?,
                    number: r.get(9)?,
                    title: r.get(10)?,
                    url: r.get(11)?,
                    released_at: r.get(12)?,
                    seen: r.get::<_, i64>(13)? != 0,
                };
                Ok((series, ep))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_creates_schema() {
        let db = Db::open(":memory:").unwrap();
        let count: i64 = db
            .conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table' AND name IN ('sources','series','episodes','settings')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 4);
    }

    #[test]
    fn upsert_source_and_series_then_follow() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "https://wwv.animeytx.net").unwrap();

        let s = crate::models::Series {
            id: 0,
            slug: "baki-dou".into(),
            title: "Baki-dou".into(),
            url: "https://wwv.animeytx.net/tv/baki-dou/".into(),
            cover_url: None,
            is_airing: true,
            followed: false,
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
    fn insert_episode_dedups_and_marks_seen() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "https://wwv.animeytx.net").unwrap();
        let s = crate::models::Series {
            id: 0, slug: "x".into(), title: "X".into(),
            url: "u".into(), cover_url: None, is_airing: true, followed: false,
        };
        let sid = db.upsert_series(src, &s).unwrap();
        db.set_followed(sid, true).unwrap();

        let ep = crate::models::Episode {
            id: 0, series_id: sid, number: "1".into(), title: None,
            url: "https://site/ep1".into(), released_at: None, seen: false,
        };
        let eid = db.insert_episode(&ep).unwrap();
        // same url again => no new row
        let eid_dup = db.insert_episode(&ep).unwrap();
        assert_eq!(eid, eid_dup);

        assert_eq!(db.pending_count().unwrap(), 1);
        db.set_seen(eid, true).unwrap();
        assert_eq!(db.pending_count().unwrap(), 0);
    }

    #[test]
    fn existing_episode_urls_returns_known_urls() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "b").unwrap();
        let s = crate::models::Series {
            id: 0, slug: "x".into(), title: "X".into(),
            url: "u".into(), cover_url: None, is_airing: true, followed: true,
        };
        let sid = db.upsert_series(src, &s).unwrap();
        db.insert_episode(&crate::models::Episode {
            id: 0, series_id: sid, number: "1".into(), title: None,
            url: "https://site/ep1".into(), released_at: None, seen: false,
        }).unwrap();
        let urls = db.existing_episode_urls(sid).unwrap();
        assert!(urls.contains("https://site/ep1"));
        assert_eq!(urls.len(), 1);
    }
}
