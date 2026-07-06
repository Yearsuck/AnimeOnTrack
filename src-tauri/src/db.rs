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
}
