use anyhow::Result;
use rusqlite::Connection;

/// Extract the leading numeric portion of an episode-number string, e.g.
/// "12" -> 12.0, "12.5" -> 12.5 (OVA/special numbering), "1x05" -> 1.0
/// (season-prefixed numbering seen on multi-cour series pages). Returns
/// `None` when there are no leading digits at all (e.g. "OVA"), so callers
/// can fall back to an exact-string match instead of guessing an ordering.
fn parse_ep_number(s: &str) -> Option<f64> {
    let trimmed = s.trim();
    let mut end = 0;
    let mut seen_dot = false;
    for (i, c) in trimmed.char_indices() {
        if c.is_ascii_digit() {
            end = i + c.len_utf8();
        } else if c == '.' && !seen_dot && end > 0 {
            seen_dot = true;
        } else {
            break;
        }
    }
    if end == 0 {
        None
    } else {
        trimmed[..end].parse().ok()
    }
}

/// Add `column` to `table` if it isn't already there. SQLite has no
/// `ALTER TABLE ... ADD COLUMN IF NOT EXISTS`, so this checks `PRAGMA
/// table_info` first — needed because `init_schema` runs on every `Db::open`
/// and must be a no-op on a DB that already has the column (the codebase has
/// no separate migration framework; `series`/`episodes` already shipped
/// before this change, so evolving their schema in place is the existing idiom).
fn ensure_column(conn: &Connection, table: &str, column: &str, coltype: &str) -> Result<()> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let has_column = stmt
        .query_map([], |r| r.get::<_, String>(1))?
        .filter_map(|r| r.ok())
        .any(|name| name == column);
    if !has_column {
        conn.execute(&format!("ALTER TABLE {table} ADD COLUMN {column} {coltype}"), [])?;
    }
    Ok(())
}

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
        // series.backlog_status: NULL (normal row) | 'want' | 'discarded'.
        // series.kind: free-text type badge ("TV"/"OVA"/"Movie"/etc), same
        // vocabulary as the adapter's FinishedCard.kind — don't validate
        // against an enum, the live site's own vocabulary is inconsistent.
        ensure_column(&self.conn, "series", "backlog_status", "TEXT")?;
        ensure_column(&self.conn, "series", "kind", "TEXT")?;
        self.conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS series_genres (
                series_id INTEGER NOT NULL REFERENCES series(id),
                genre TEXT NOT NULL,
                PRIMARY KEY(series_id, genre)
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

    pub fn get_source_base_url(&self, source_id: i64) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row("SELECT base_url FROM sources WHERE id=?1", [source_id], |r| {
                r.get::<_, String>(0)
            })
            .ok())
    }

    pub fn get_series_url(&self, series_id: i64) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row("SELECT url FROM series WHERE id=?1", [series_id], |r| {
                r.get::<_, String>(0)
            })
            .ok())
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

    /// Replace a series' cover with a fetched base64 data URI.
    pub fn update_series_cover(&self, series_id: i64, cover_url: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE series SET cover_url=?1 WHERE id=?2",
            (cover_url, series_id),
        )?;
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

    pub fn set_backlog_status(&self, series_id: i64, status: Option<&str>) -> Result<()> {
        self.conn
            .execute("UPDATE series SET backlog_status=?1 WHERE id=?2", (status, series_id))?;
        Ok(())
    }

    pub fn get_backlog_status(&self, series_id: i64) -> Result<Option<String>> {
        Ok(self.conn.query_row(
            "SELECT backlog_status FROM series WHERE id=?1",
            [series_id],
            |r| r.get::<_, Option<String>>(0),
        )?)
    }

    pub fn set_kind(&self, series_id: i64, kind: &str) -> Result<()> {
        self.conn
            .execute("UPDATE series SET kind=?1 WHERE id=?2", (kind, series_id))?;
        Ok(())
    }

    /// Insert genres for a series; already-present (series_id, genre) pairs
    /// are left alone (genres are static once fetched, no need to delete).
    pub fn insert_series_genres(&self, series_id: i64, genres: &[String]) -> Result<()> {
        for g in genres {
            self.conn.execute(
                "INSERT INTO series_genres(series_id, genre) VALUES(?1, ?2)
                 ON CONFLICT(series_id, genre) DO NOTHING",
                (series_id, g),
            )?;
        }
        Ok(())
    }

    pub fn list_series_genres(&self, series_id: i64) -> Result<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT genre FROM series_genres WHERE series_id=?1 ORDER BY genre")?;
        let rows = stmt
            .query_map([series_id], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Every URL that already has a `series` row for this source — any
    /// `backlog_status` or `followed` value counts as "already decided", so
    /// the swipe deck never re-offers a card the user has already acted on.
    pub fn known_series_urls(&self, source_id: i64) -> Result<std::collections::HashSet<String>> {
        let mut stmt = self.conn.prepare("SELECT url FROM series WHERE source_id=?1")?;
        let urls = stmt
            .query_map([source_id], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<std::collections::HashSet<_>>>()?;
        Ok(urls)
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

    /// Followed series with their episode counts (total, seen) and the most
    /// recent episode's `added_at`, for the library view. Series with zero
    /// scraped episodes still appear (`total=0`).
    pub fn list_library(&self, source_id: i64) -> Result<Vec<crate::models::LibraryItem>> {
        let mut stmt = self.conn.prepare(
            "SELECT s.id, s.slug, s.title, s.url, s.cover_url, s.is_airing, s.followed,
                    COUNT(e.id) AS total,
                    SUM(CASE WHEN e.seen=1 THEN 1 ELSE 0 END) AS seen,
                    MAX(e.added_at) AS last_added
             FROM series s
             LEFT JOIN episodes e ON e.series_id = s.id
             WHERE s.source_id=?1 AND s.followed=1
             GROUP BY s.id
             ORDER BY s.title",
        )?;
        let rows = stmt
            .query_map([source_id], |r| {
                let series = Self::row_to_series(r)?;
                Ok(crate::models::LibraryItem {
                    series,
                    total_episodes: r.get("total")?,
                    seen_episodes: r.get::<_, Option<i64>>("seen")?.unwrap_or(0),
                    last_added: r.get("last_added")?,
                })
            })?
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

    /// Enforce sequential watching: marking an episode seen also marks every
    /// earlier episode of that series seen (no gaps like "watched 10 but not
    /// 6-9"); un-marking an episode also un-marks every later one (you can't
    /// have watched what comes after something you're un-marking).
    ///
    /// Comparison happens in Rust via `parse_ep_number`, not SQLite's
    /// `CAST(... AS INTEGER)` — the two disagreed on numbers like "1x05"
    /// (season-prefixed) or "12.5" (specials): SQLite's cast still read a
    /// leading digit while Rust's old strict `i64::parse` failed to 0,
    /// which made un-marking such an episode wipe the *whole series'*
    /// watched history instead of just the later episodes.
    pub fn set_seen_cascade(&self, series_id: i64, number: &str, seen: bool) -> Result<()> {
        let Some(target) = parse_ep_number(number) else {
            // No leading digits at all: ordering is meaningless, so just
            // toggle the exact-matching episode(s) rather than cascade.
            self.conn.execute(
                "UPDATE episodes SET seen=?1 WHERE series_id=?2 AND number=?3",
                (seen as i64, series_id, number),
            )?;
            return Ok(());
        };
        let mut stmt = self
            .conn
            .prepare("SELECT id, number FROM episodes WHERE series_id=?1")?;
        let rows: Vec<(i64, String)> = stmt
            .query_map([series_id], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for (id, num) in rows {
            let matches = match parse_ep_number(&num) {
                Some(v) if seen => v <= target,
                Some(v) => v >= target,
                None => false,
            };
            if matches {
                self.conn
                    .execute("UPDATE episodes SET seen=?1 WHERE id=?2", (seen as i64, id))?;
            }
        }
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
                "SELECT count(*) FROM sqlite_master WHERE type='table' AND name IN ('sources','series','episodes','settings','series_genres')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 5);
    }

    #[test]
    fn series_table_has_backlog_status_and_kind_columns() {
        let db = Db::open(":memory:").unwrap();
        let mut stmt = db.conn.prepare("PRAGMA table_info(series)").unwrap();
        let cols: Vec<String> = stmt
            .query_map([], |r| r.get::<_, String>(1))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert!(cols.contains(&"backlog_status".to_string()));
        assert!(cols.contains(&"kind".to_string()));
    }

    #[test]
    fn migration_is_idempotent_on_an_existing_on_disk_db() {
        let path = std::env::temp_dir().join(format!("aot_migration_test_{}.sqlite", std::process::id()));
        let path_str = path.to_str().unwrap();
        let _ = std::fs::remove_file(&path);
        {
            Db::open(path_str).unwrap();
        }
        // Second open must not error even though backlog_status/kind/series_genres
        // already exist from the first open (ALTER TABLE ADD COLUMN errors if run
        // twice unguarded).
        let db = Db::open(path_str).unwrap();
        let count: i64 = db
            .conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='series_genres'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
        let _ = std::fs::remove_file(&path);
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
    fn seen_cascade_handles_non_integer_episode_numbers() {
        // Regression: episode numbers like "1x05" (season-prefixed) used to
        // fail Rust's strict i64 parse (-> 0) while SQLite's CAST still read
        // the leading digit, so un-marking one used to wipe every episode's
        // seen flag in the series instead of just the later ones.
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "https://wwv.animeytx.net").unwrap();
        let s = crate::models::Series {
            id: 0, slug: "x".into(), title: "X".into(),
            url: "u".into(), cover_url: None, is_airing: true, followed: false,
        };
        let sid = db.upsert_series(src, &s).unwrap();

        let mk = |n: &str, url: &str| crate::models::Episode {
            id: 0, series_id: sid, number: n.into(), title: None,
            url: url.into(), released_at: None, seen: false,
        };
        db.insert_episode(&mk("1x01", "https://site/e1")).unwrap();
        db.insert_episode(&mk("1x02", "https://site/e2")).unwrap();
        db.insert_episode(&mk("1x03", "https://site/e3")).unwrap();

        // marking "1x02" seen cascades to "1x01" too, but not "1x03"
        db.set_seen_cascade(sid, "1x02", true).unwrap();
        let eps = db.list_series_episodes(sid).unwrap();
        assert!(eps.iter().find(|e| e.number == "1x01").unwrap().seen);
        assert!(eps.iter().find(|e| e.number == "1x02").unwrap().seen);
        assert!(!eps.iter().find(|e| e.number == "1x03").unwrap().seen);

        // un-marking "1x02" un-marks "1x02"/"1x03" but must leave "1x01" alone
        db.set_seen_cascade(sid, "1x02", false).unwrap();
        let eps = db.list_series_episodes(sid).unwrap();
        assert!(eps.iter().find(|e| e.number == "1x01").unwrap().seen);
        assert!(!eps.iter().find(|e| e.number == "1x02").unwrap().seen);
        assert!(!eps.iter().find(|e| e.number == "1x03").unwrap().seen);
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

    #[test]
    fn series_genres_insert_is_idempotent_and_lists_sorted() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "b").unwrap();
        let s = crate::models::Series {
            id: 0, slug: "x".into(), title: "X".into(),
            url: "u".into(), cover_url: None, is_airing: false, followed: false,
        };
        let sid = db.upsert_series(src, &s).unwrap();

        db.insert_series_genres(sid, &["Seinen".to_string(), "Drama".to_string()]).unwrap();
        // inserting the same genres again must not error or duplicate
        db.insert_series_genres(sid, &["Drama".to_string()]).unwrap();

        let genres = db.list_series_genres(sid).unwrap();
        assert_eq!(genres, vec!["Drama".to_string(), "Seinen".to_string()]);
    }

    #[test]
    fn backlog_status_and_kind_round_trip() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "b").unwrap();
        let s = crate::models::Series {
            id: 0, slug: "x".into(), title: "X".into(),
            url: "u".into(), cover_url: None, is_airing: false, followed: false,
        };
        let sid = db.upsert_series(src, &s).unwrap();

        assert_eq!(db.get_backlog_status(sid).unwrap(), None);
        db.set_backlog_status(sid, Some("want")).unwrap();
        assert_eq!(db.get_backlog_status(sid).unwrap(), Some("want".to_string()));
        db.set_kind(sid, "TV").unwrap();

        // start_watching's transition: 'want' -> followed, backlog_status cleared
        db.set_followed(sid, true).unwrap();
        db.set_backlog_status(sid, None).unwrap();
        assert_eq!(db.get_backlog_status(sid).unwrap(), None);
        assert!(db.list_followed(src).unwrap().iter().any(|f| f.id == sid));
    }

    #[test]
    fn known_series_urls_reflects_any_decided_row() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "b").unwrap();
        let s = crate::models::Series {
            id: 0, slug: "x".into(), title: "X".into(),
            url: "https://site/tv/x/".into(), cover_url: None, is_airing: false, followed: false,
        };
        let sid = db.upsert_series(src, &s).unwrap();
        db.set_backlog_status(sid, Some("discarded")).unwrap();

        let known = db.known_series_urls(src).unwrap();
        assert!(known.contains("https://site/tv/x/"));
        assert_eq!(known.len(), 1);
    }
}
