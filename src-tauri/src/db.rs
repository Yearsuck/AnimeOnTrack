use anyhow::Result;
use rusqlite::Connection;
use rusqlite::OptionalExtension;
use serde::Deserialize;



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
        // anilist_id: real numeric AniList id for `series` rows created from
        // a catalog swipe decision (slug `anilist-{id}`). Added so the
        // catalog swipe picker can exclude already-decided titles with a
        // plain indexed-friendly `NOT IN` instead of parsing it back out of
        // the slug on every query. Backfilled below from any pre-existing
        // `anilist-{id}` slug so rows written before this column existed
        // still get excluded correctly.
        ensure_column(&self.conn, "series", "anilist_id", "INTEGER")?;
        self.conn.execute(
            "UPDATE series SET anilist_id = CAST(SUBSTR(slug, 9) AS INTEGER)
             WHERE slug LIKE 'anilist-%' AND anilist_id IS NULL",
            [],
        )?;
        // watched_externally: set by decide_catalog_card's "seen" decision —
        // "I've watched this outside the app, don't show it to me again,
        // don't put it in my backlog." No episode data backs this (AniList
        // never hosts video), so it's a flag, not real progress tracking.
        ensure_column(&self.conn, "series", "watched_externally", "INTEGER DEFAULT 0")?;
        // next_episode_at: unix timestamp of the next episode's release,
        // parsed from the airing listing's `data-rlsdt`; site_episode_count:
        // the site's own reported episode count (`.sb` badge), NULL when
        // non-numeric (e.g. "??"). Both scan-owned (see `upsert_series`) —
        // added for the scraper-performance skip logic (optimization A) and
        // the airing grid's newest-first default sort. last_checked_at: ISO
        // 8601 timestamp of the last time a followed series NOT on the
        // current airing listing (a finished show) had its episode list
        // actually fetched — gates the `FINISHED_RECHECK` interval in
        // `refresh()` so a completed series isn't re-fetched every refresh.
        ensure_column(&self.conn, "series", "next_episode_at", "INTEGER")?;
        ensure_column(&self.conn, "series", "site_episode_count", "INTEGER")?;
        ensure_column(&self.conn, "series", "last_checked_at", "TEXT")?;
        // series.carried_seen_number: cross-site follow carry-over (see
        // docs/superpowers/specs/2026-07-12-cross-site-follow-carryover-design.md).
        // When switching sites, a followed series matched by title on the new
        // site gets its follow carried and this set to the highest *seen*
        // episode number from the old site. `refresh()` applies it once (via
        // `set_seen_cascade`) the first time the new site's episodes are
        // fetched, then clears it back to NULL — so it never re-forces "seen".
        ensure_column(&self.conn, "series", "carried_seen_number", "INTEGER")?;
        // episodes.seen_at: ISO 8601 timestamp set by `set_seen`/
        // `set_seen_cascade` when an episode is marked seen, cleared (NULL)
        // when un-marked — including every row a cascade touches, not just
        // the one explicitly toggled. Distinct from `added_at` (when the row
        // was scraped): this is when the user actually watched it, and is
        // the only thing that can drive "continue watching, most recent
        // first" in the library view (see the library-redesign design doc).
        ensure_column(&self.conn, "episodes", "seen_at", "TEXT")?;
        self.conn.execute_batch(
            "CREATE INDEX IF NOT EXISTS idx_series_next_ep ON series(source_id, is_airing, next_episode_at);",
        )?;
        // site_id: stable adapter slug (e.g. "animeytx") — see
        // `adapter::SiteInfo`/`adapter::adapter_for`. Never the base_url,
        // which changes with every mirror. Before this column existed there
        // was exactly one supported site, so every pre-existing row is
        // backfilled to "animeytx" rather than left NULL (verified via
        // `SELECT * FROM sources` before writing this migration — one row).
        ensure_column(&self.conn, "sources", "site_id", "TEXT")?;
        self.conn.execute(
            "UPDATE sources SET site_id = 'animeytx' WHERE site_id IS NULL",
            [],
        )?;
        self.conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS series_genres (
                series_id INTEGER NOT NULL REFERENCES series(id),
                genre TEXT NOT NULL,
                PRIMARY KEY(series_id, genre)
            );
            "#,
        )?;
        // Local mirror of AniList's catalog (see anilist.rs / sync_anime_catalog)
        // — browsing and Descubrir's "Catálogo completo" source read from this
        // table, not from AniList live, once synced. `sort_order` is kept as
        // the within-sync sequence items were written in, but display
        // ordering reads `popularity` instead — the sync now crawls
        // partitioned by id/date, not by popularity, so `sort_order` no
        // longer encodes popularity (see anilist.rs module docs).
        self.conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS anilist_catalog (
                id INTEGER PRIMARY KEY,
                title TEXT NOT NULL,
                cover_url TEXT,
                format TEXT,
                episodes INTEGER,
                average_score INTEGER,
                url TEXT NOT NULL,
                sort_order INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS anilist_catalog_genres (
                anilist_id INTEGER NOT NULL REFERENCES anilist_catalog(id),
                genre TEXT NOT NULL,
                PRIMARY KEY(anilist_id, genre)
            );
            "#,
        )?;
        ensure_column(&self.conn, "anilist_catalog", "popularity", "INTEGER")?;
        // title_romaji/title_english: kept alongside `title` (which collapses
        // to english-or-romaji) so matching.rs's best_match can try both when
        // looking a catalog title up on the scraped site — see
        // `anilist::CatalogAnime`'s field comments. Existing rows backfill on
        // the next incremental sync; NULL until then is expected, not an error.
        ensure_column(&self.conn, "anilist_catalog", "title_romaji", "TEXT")?;
        ensure_column(&self.conn, "anilist_catalog", "title_english", "TEXT")?;
        // AniList's own status vocabulary (RELEASING/NOT_YET_RELEASED/...) —
        // NULL until the next sync backfills it. See
        // db/catalog.rs::random_catalog_anime_in_genre's hide_upcoming param.
        ensure_column(&self.conn, "anilist_catalog", "status", "TEXT")?;
        // AniList's real per-episode minutes, when it has one. NULL for rows
        // synced before this column existed (backfills on the next sync) or
        // when AniList itself has no duration for the title — falls back to
        // db/stats.rs::minutes_per_episode's estimate in that case.
        ensure_column(&self.conn, "anilist_catalog", "duration", "INTEGER")?;
        // First `isMain` studio's name, when AniList has one. NULL for rows
        // synced before this column existed (backfills on the next sync) or
        // when AniList reports no credited studio at all. Co-productions with
        // multiple mains only keep the first — see
        // `anilist::CatalogAnime::studio`'s doc comment.
        ensure_column(&self.conn, "anilist_catalog", "studio", "TEXT")?;
        // Real premiere date (Unix ts, midnight UTC), when AniList has a
        // fully-specified one. NULL for rows synced before this column
        // existed or a fuzzy/partial AniList date — see
        // `anilist::FuzzyDate::to_timestamp`'s doc comment. Used by
        // `db::episodes::airing_season_dates` to answer "aired this season"
        // for airing-site rows with no scraped episode data.
        ensure_column(&self.conn, "anilist_catalog", "start_date", "INTEGER")?;
        self.conn.execute_batch(
            "CREATE INDEX IF NOT EXISTS idx_catalog_popularity ON anilist_catalog(popularity DESC);
             CREATE INDEX IF NOT EXISTS idx_catalog_genre ON anilist_catalog_genres(genre);",
        )?;

        // Site-agnostic library (docs/cross-site-library-investigation.md,
        // option C). Identity is canonical (AniList id, else normalized title),
        // so the same show followed on two sites is one entry and a site outage
        // never strands your library. Progress is a single seen-watermark,
        // which is lossless because watching is gap-free.
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS library (
                id INTEGER PRIMARY KEY,
                canon_key TEXT NOT NULL UNIQUE,
                anilist_id INTEGER,
                display_title TEXT NOT NULL,
                followed INTEGER NOT NULL DEFAULT 0,
                backlog_status TEXT,
                watched_externally INTEGER NOT NULL DEFAULT 0,
                seen_watermark INTEGER NOT NULL DEFAULT 0
            );",
        )?;

        // One-time repair for the episode-duplication bug. The target sites
        // occasionally move to a new domain (e.g. wwv.animeytx.net began
        // 301-ing to animeyt.cc, with a different URL path shape). Episodes
        // were de-duplicated on their *full URL*, so a domain change made every
        // episode look new: the scan re-inserted a second, unseen copy of every
        // episode — doubling watched series into half-seen/half-unseen and
        // flooding the pending queue with hundreds of bogus episodes.
        //
        // This collapses each (series_id, number) group back to a single row,
        // keeping "seen" if *any* copy in the group was seen (so watched
        // progress is never lost), and keeping the lowest-id row as the
        // survivor. The scan path now de-duplicates by number (not URL), so it
        // can't recur; and the next scan refreshes the survivor's URL to the
        // current domain. Guarded by a settings flag so it runs exactly once.
        if self.get_setting("episode_dedup_by_number_v1")?.is_none() {
            self.conn.execute_batch(
                "UPDATE episodes SET seen = 1, seen_at = COALESCE(seen_at, datetime('now'))
                   WHERE id IN (
                     SELECT MIN(id) FROM episodes GROUP BY series_id, number HAVING MAX(seen) = 1
                   );
                 DELETE FROM episodes WHERE id NOT IN (
                   SELECT MIN(id) FROM episodes GROUP BY series_id, number
                 );",
            )?;
            self.set_setting("episode_dedup_by_number_v1", "1")?;
        }

        // Rebuild the canonical library projection from the current per-site
        // follows. Cheap (hundreds of rows) and idempotent, so running it on
        // every open keeps it in sync while `series` is still the source of
        // truth (phase 1); a later phase makes `library` authoritative and
        // gates this.
        self.sync_library_from_series()?;

        Ok(())
    }

    /// Write a consistent single-file copy of the whole database using
    /// SQLite's `VACUUM INTO` — safe to read even with this connection open
    /// (unlike copying the file bytes, which can catch a torn WAL/journal).
    pub fn snapshot_to(&self, path: &str) -> Result<()> {
        // VACUUM INTO refuses to overwrite an existing file.
        let _ = std::fs::remove_file(path);
        self.conn.execute("VACUUM INTO ?1", [path])?;
        Ok(())
    }
}

mod sources;
mod settings;
mod episodes;
mod catalog;
pub(crate) mod stats;
mod airing;
mod series;
pub(crate) mod library;

pub use series::SwipeHistoryRow;
pub use catalog::CatalogFilter;
pub use airing::PendingSort;

#[cfg(test)]
mod test_support;

#[cfg(test)]
mod tests {
    use super::*;
    use super::test_support::*;
    use super::stats::franchise_key;

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
    fn anilist_id_is_backfilled_from_synthetic_slug_on_migration() {
        let path = std::env::temp_dir().join(format!("aot_anilist_id_migration_test_{}.sqlite", std::process::id()));
        let path_str = path.to_str().unwrap();
        let _ = std::fs::remove_file(&path);
        {
            let db = Db::open(path_str).unwrap();
            let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
            // Simulate a pre-migration row: slug carries the anilist id but
            // the anilist_id column (added by this migration) is unset.
            let s = crate::models::Series {
                id: 0, slug: "anilist-777".into(), title: "Legacy".into(),
                url: "u".into(), cover_url: None, is_airing: false, followed: false, next_episode_at: None, site_episode_count: None,
            };
            db.upsert_series(src, &s).unwrap();
        }
        // Re-opening re-runs init_schema/migration on the same on-disk DB.
        let db = Db::open(path_str).unwrap();
        let anilist_id: Option<i64> = db
            .conn
            .query_row("SELECT anilist_id FROM series WHERE slug='anilist-777'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(anilist_id, Some(777));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn duplicate_episodes_from_a_domain_change_are_collapsed_keeping_seen() {
        let path = std::env::temp_dir()
            .join(format!("aot_ep_dedup_migration_test_{}.sqlite", std::process::id()));
        let path_str = path.to_str().unwrap();
        let _ = std::fs::remove_file(&path);
        let sid;
        {
            let db = Db::open(path_str).unwrap();
            let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
            sid = db.upsert_series(src, &mk_airing("x", "X", None)).unwrap();
            let ep = |number: &str, url: &str, seen: bool| crate::models::Episode {
                id: 0, series_id: sid, number: number.into(), title: None,
                url: url.into(), released_at: None, seen,
            };
            // The bug's exact shape: episode 1 seen on the old domain, then a
            // second unseen copy on the new domain; episode 2 unseen on both.
            db.insert_episode(&ep("1", "https://wwv.animeytx.net/anime/x-capitulo-1/", true)).unwrap();
            db.insert_episode(&ep("1", "https://animeyt.cc/999/anime/x-capitulo-1/", false)).unwrap();
            db.insert_episode(&ep("2", "https://wwv.animeytx.net/anime/x-capitulo-2/", false)).unwrap();
            db.insert_episode(&ep("2", "https://animeyt.cc/998/anime/x-capitulo-2/", false)).unwrap();
            // Clear the one-time flag so re-opening re-runs the dedup migration.
            db.conn
                .execute("DELETE FROM settings WHERE key='episode_dedup_by_number_v1'", [])
                .unwrap();
        }
        let db = Db::open(path_str).unwrap();
        let mut stmt = db
            .conn
            .prepare("SELECT number, seen FROM episodes WHERE series_id=?1 ORDER BY number")
            .unwrap();
        let rows: Vec<(String, i64)> = stmt
            .query_map([sid], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        // One row per episode; episode 1 stays seen, episode 2 stays unseen.
        assert_eq!(rows, vec![("1".to_string(), 1), ("2".to_string(), 0)]);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn followed_titles_with_watermark_excludes_active_source_and_computes_seen_high_water() {
        let db = Db::open(":memory:").unwrap();
        let src_a = db.upsert_source("A", "a", "animeytx").unwrap();
        let src_b = db.upsert_source("B", "b", "tioanime").unwrap();

        // src_a: one followed with eps 1..5 seen up to 3, one followed with a
        // recap row + nothing seen, and one UN-followed (must be excluded).
        let a1 = db.upsert_series(src_a, &mk_airing("frieren", "Frieren", None)).unwrap();
        db.set_followed(a1, true).unwrap();
        insert_eps_seen_up_to(&db, a1, 5, 3);

        let a2 = db.upsert_series(src_a, &mk_airing("bocchi", "Bocchi the Rock!", None)).unwrap();
        db.set_followed(a2, true).unwrap();
        db.insert_episode(&crate::models::Episode {
            id: 0, series_id: a2, number: "0 | Recap".into(), title: None,
            url: "https://site/a2-recap".into(), released_at: None, seen: false,
        }).unwrap();

        let a3 = db.upsert_series(src_a, &mk_airing("unfollowed", "Unfollowed Show", None)).unwrap();
        let _ = a3;

        // src_b has its own followed series — must NOT come back when we
        // exclude src_b.
        let b1 = db.upsert_series(src_b, &mk_airing("frieren", "Frieren", None)).unwrap();
        db.set_followed(b1, true).unwrap();

        let mut got = db.followed_titles_with_watermark(src_b).unwrap();
        got.sort();
        assert_eq!(
            got,
            vec![("Bocchi the Rock!".to_string(), 0), ("Frieren".to_string(), 3)]
        );
    }

    #[test]
    fn franchise_key_collapses_seasons_and_parts() {
        // Season suffix collapses onto the base.
        assert_eq!(
            franchise_key("Tensei shitara Slime Datta Ken Temporada 4"),
            franchise_key("Tensei shitara Slime Datta Ken")
        );
        // Roman-numeral season collapses onto the base.
        assert_eq!(franchise_key("Overlord IV"), franchise_key("Overlord"));
        // English "Season N" and "Nth Season" both collapse.
        assert_eq!(franchise_key("Vinland Saga Season 2"), franchise_key("Vinland Saga"));
        assert_eq!(franchise_key("Bocchi the Rock! 2nd Season"), franchise_key("Bocchi the Rock!"));
    }

    #[test]
    fn snapshot_to_produces_valid_sqlite() {
        let db = Db::open(":memory:").unwrap();
        let dir = std::env::temp_dir();
        let out = dir.join(format!("aot_snap_{}.sqlite", std::process::id()));
        db.snapshot_to(out.to_str().unwrap()).unwrap();
        // Re-open the snapshot and integrity-check it.
        let conn = rusqlite::Connection::open(&out).unwrap();
        let ok: String = conn.query_row("PRAGMA integrity_check", [], |r| r.get(0)).unwrap();
        assert_eq!(ok, "ok");
        let has_sources: i64 = conn
            .query_row("SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='sources'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(has_sources, 1);
        drop(conn);
        std::fs::remove_file(&out).ok();
    }
}
