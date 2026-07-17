use anyhow::Result;
use rusqlite::types::Value;
use rusqlite::Connection;
use rusqlite::OptionalExtension;
use serde::Deserialize;

/// The subset of a `series` row's fields the swipe-history strip needs —
/// see `Db::get_series_for_history` and `commands::list_swipe_history`.
#[derive(Debug, Clone, PartialEq)]
pub struct SwipeHistoryRow {
    pub title: String,
    pub poster_url: Option<String>,
    pub backlog_status: Option<String>,
    pub watched_externally: bool,
    /// The row's `series.url` — lets the frontend clear this card from its
    /// client-side decided-set when it legitimately returns to the deck
    /// (undo / return-to-deck). See the design spec's Fix 1.
    pub url: String,
}

/// The subset of a `series` row's fields `link_catalog_series` needs to
/// decide how (or whether) to link it — see `Db::get_series_for_link` and
/// `commands::link_catalog_series`.
#[derive(Debug, Clone, PartialEq)]
pub struct SeriesForLink {
    pub id: i64,
    pub source_id: i64,
    pub slug: String,
    pub anilist_id: Option<i64>,
    pub followed: bool,
    pub backlog_status: Option<String>,
    pub watched_externally: bool,
}

impl SeriesForLink {
    /// Idempotent early-out for `commands::link_series_core`: a row is
    /// "already linked" either because it was never a catalog row to begin
    /// with (`anilist_id` NULL — a plain site row, e.g. from the airing
    /// scan) *or* because a previous `link_catalog_series` call already
    /// rewrote it onto a real site slug (`relink_series` deliberately keeps
    /// `anilist_id` set after a successful link — only slug/url/cover/kind
    /// change — so `anilist_id.is_some()` alone is NOT a reliable "still
    /// needs linking" signal). Checking the slug too closes that gap: the
    /// three trigger call sites (Seen swipe, "Empezar a ver", opening
    /// SeriesDetail) can race on the same freshly-linked row, and without
    /// this each would re-search and re-scrape it. Mirrors the frontend's
    /// `isUnlinkedCatalogRow` (`src/lib/catalogLink.ts`) — keep both in sync.
    pub fn already_linked_to_site(&self) -> bool {
        self.anilist_id.is_none() || !self.slug.starts_with("anilist-")
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
        self.conn.execute_batch(
            "CREATE INDEX IF NOT EXISTS idx_catalog_popularity ON anilist_catalog(popularity DESC);
             CREATE INDEX IF NOT EXISTS idx_catalog_genre ON anilist_catalog_genres(genre);",
        )?;
        Ok(())
    }


    pub fn get_series_url(&self, series_id: i64) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row("SELECT url FROM series WHERE id=?1", [series_id], |r| {
                r.get::<_, String>(0)
            })
            .ok())
    }

    /// The subset of a `series` row's fields `link_catalog_series` needs to
    /// decide how (or whether) to link it — see `commands::link_catalog_series`.
    pub fn get_series_for_link(&self, series_id: i64) -> Result<Option<SeriesForLink>> {
        Ok(self
            .conn
            .query_row(
                "SELECT id, source_id, slug, anilist_id, followed, backlog_status, watched_externally
                 FROM series WHERE id=?1",
                [series_id],
                |r| {
                    Ok(SeriesForLink {
                        id: r.get(0)?,
                        source_id: r.get(1)?,
                        slug: r.get(2)?,
                        anilist_id: r.get(3)?,
                        followed: r.get::<_, i64>(4)? != 0,
                        backlog_status: r.get(5)?,
                        watched_externally: r.get::<_, i64>(6)? != 0,
                    })
                },
            )
            .ok())
    }

    /// Does a *different* `series` row already own `(source_id, slug)`? Used
    /// by `link_catalog_series` to detect that the matched site series is
    /// already tracked (e.g. it's on the airing list) before ever attempting
    /// an insert/update that could violate `UNIQUE(source_id, slug)`.
    pub fn find_series_id_by_slug(&self, source_id: i64, slug: &str, exclude_id: i64) -> Result<Option<i64>> {
        Ok(self
            .conn
            .query_row(
                "SELECT id FROM series WHERE source_id=?1 AND slug=?2 AND id != ?3",
                (source_id, slug, exclude_id),
                |r| r.get(0),
            )
            .ok())
    }

    /// Fold a synthetic catalog-swipe row's decision flags onto an existing
    /// real site row that turns out to share its slug, then delete the
    /// synthetic row. `followed`/`watched_externally` are OR'd (either source
    /// wanting it means the merged row does); `backlog_status` is
    /// most-specific-wins — the existing row's own status survives if it
    /// already has one, otherwise the synthetic row's takes over. Slug/url/
    /// cover/kind/episodes are untouched: the existing row is canonical
    /// (already scraped through the normal path), not the synthetic one.
    pub fn merge_series_into(&self, existing_id: i64, synthetic_id: i64) -> Result<()> {
        let (followed, backlog_status, watched_externally): (i64, Option<String>, i64) = self.conn.query_row(
            "SELECT followed, backlog_status, watched_externally FROM series WHERE id=?1",
            [synthetic_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?;
        self.conn.execute(
            "UPDATE series SET
                followed = followed OR ?1,
                watched_externally = watched_externally OR ?2,
                backlog_status = COALESCE(backlog_status, ?3)
             WHERE id=?4",
            (followed, watched_externally, backlog_status, existing_id),
        )?;
        self.delete_series(synthetic_id)
    }

    /// Rewrite a synthetic row's slug/url/cover/kind in place after a
    /// successful site match — `id`/`followed`/`backlog_status`/
    /// `watched_externally`/`anilist_id` are deliberately untouched (see
    /// `commands::link_catalog_series`).
    pub fn relink_series(
        &self,
        series_id: i64,
        slug: &str,
        url: &str,
        cover_url: Option<&str>,
        kind: &str,
    ) -> Result<()> {
        self.conn.execute(
            "UPDATE series SET slug=?1, url=?2, cover_url=?3, kind=?4 WHERE id=?5",
            (slug, url, cover_url, kind, series_id),
        )?;
        Ok(())
    }

    /// Replace a series' genre set outright (unlike `insert_series_genres`,
    /// which is additive) — used when linking rewrites a synthetic row's
    /// AniList-sourced genres with the site's own.
    pub fn replace_series_genres(&self, series_id: i64, genres: &[String]) -> Result<()> {
        self.conn.execute("DELETE FROM series_genres WHERE series_id=?1", [series_id])?;
        self.insert_series_genres(series_id, genres)
    }


    /// `next_episode_at`/`site_episode_count` are written here just like the
    /// rest of the scanned fields (unlike `followed`, which is deliberately
    /// excluded from the UPDATE so re-scanning never un-follows anything) —
    /// see the scraper-performance design doc. Every real caller besides the
    /// airing scan (`decide_swipe`, `decide_catalog_card`) always passes
    /// `None` for both, and never collides with an already-scanned airing
    /// row's `(source_id, slug)` in practice (the swipe/catalog decks only
    /// ever offer titles not already known — see `known_series_urls`), so
    /// this can't silently clobber real scanned data in the normal flow.
    pub fn upsert_series(&self, source_id: i64, s: &crate::models::Series) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO series(source_id, slug, title, url, cover_url, is_airing, next_episode_at, site_episode_count)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(source_id, slug) DO UPDATE SET
                title=excluded.title, url=excluded.url,
                cover_url=excluded.cover_url, is_airing=excluded.is_airing,
                next_episode_at=excluded.next_episode_at, site_episode_count=excluded.site_episode_count",
            (
                source_id, &s.slug, &s.title, &s.url,
                &s.cover_url, s.is_airing as i64,
                s.next_episode_at, s.site_episode_count,
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

    /// Every followed series on a source OTHER than `exclude_source_id`, with
    /// its title and the highest *seen* episode number (0 when nothing's been
    /// watched). Non-numeric / recap episode numbers never inflate the
    /// watermark (`CAST(number AS INTEGER)` parses leading digits, "Recap"→0).
    /// Powers cross-site follow carry-over: matched by title against the newly
    /// scanned site's airing list. See the carry-over design doc.
    pub fn followed_titles_with_watermark(
        &self,
        exclude_source_id: i64,
    ) -> Result<Vec<(String, i64)>> {
        let mut stmt = self.conn.prepare(
            "SELECT s.title,
                    COALESCE(MAX(CASE WHEN e.seen=1 THEN CAST(e.number AS INTEGER) END), 0) AS watermark
             FROM series s
             LEFT JOIN episodes e ON e.series_id = s.id
             WHERE s.followed=1 AND s.source_id <> ?1
             GROUP BY s.id
             ORDER BY s.title",
        )?;
        let rows = stmt
            .query_map([exclude_source_id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Carry a follow onto a new-site series row: set it followed and stash
    /// the watermark for `refresh()` to apply once. Guarded to `followed=0` so
    /// it never overrides an existing follow (or its real progress) — a series
    /// the user already follows on this site is left completely untouched.
    /// Returns true if a row was actually carried.
    pub fn carry_follow(&self, series_id: i64, watermark: i64) -> Result<bool> {
        let n = self.conn.execute(
            "UPDATE series SET followed=1, carried_seen_number=?2
             WHERE id=?1 AND followed=0",
            (series_id, watermark),
        )?;
        Ok(n > 0)
    }

    /// Read and clear a series' pending carry-over watermark (apply-once). The
    /// clear happens whether or not the caller ends up cascading, so a series
    /// with no episodes yet doesn't get stuck re-applying every refresh.
    pub fn take_carried_seen_number(&self, series_id: i64) -> Result<Option<i64>> {
        let v: Option<i64> = self.conn.query_row(
            "SELECT carried_seen_number FROM series WHERE id=?1",
            [series_id],
            |r| r.get(0),
        )?;
        if v.is_some() {
            self.conn.execute(
                "UPDATE series SET carried_seen_number=NULL WHERE id=?1",
                [series_id],
            )?;
        }
        Ok(v)
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

    /// Set the real numeric AniList id on a `series` row created from a
    /// catalog swipe decision — see the `anilist_id` column comment in
    /// `init_schema`.
    pub fn set_anilist_id(&self, series_id: i64, anilist_id: i64) -> Result<()> {
        self.conn
            .execute("UPDATE series SET anilist_id=?1 WHERE id=?2", (anilist_id, series_id))?;
        Ok(())
    }

    /// Set (or clear) the "watched outside the app" flag — see
    /// `watched_externally`'s column comment in `init_schema`.
    pub fn set_watched_externally(&self, series_id: i64, watched: bool) -> Result<()> {
        self.conn.execute(
            "UPDATE series SET watched_externally=?1 WHERE id=?2",
            (watched as i64, series_id),
        )?;
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


    /// `true` if this series has no `series_genres` rows yet (needs a
    /// detail-page fetch to backfill). Split out as a plain sync check so the
    /// async fetch decision can be unit-tested without a scraper/AppHandle.
    pub fn series_needs_genre_backfill(&self, series_id: i64) -> Result<bool> {
        Ok(self.list_series_genres(series_id)?.is_empty())
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
            next_episode_at: r.get("next_episode_at")?,
            site_episode_count: r.get("site_episode_count")?,
        })
    }

    /// Followed OR watched-externally series with their episode counts
    /// (total, seen), the most recent episode's `added_at`, and (via
    /// `next_unseen_episode`) the lowest-numbered unseen episode plus
    /// `last_watched_at`, for the library view. Series with zero scraped
    /// episodes still appear (`total=0`, `next_episode=None`) — status
    /// derivation on the frontend treats that as "plan", never dividing by
    /// zero, UNLESS `watched_externally=1` (a catalog "Ya lo vi" swipe never
    /// scrapes episodes), in which case the frontend classifies it as
    /// completed instead. `followed=1 AND watched_externally=1` rows are not
    /// duplicated — `GROUP BY s.id` collapses them to one row regardless of
    /// which condition matched.
    pub fn list_library(&self, source_id: i64) -> Result<Vec<crate::models::LibraryItem>> {
        let mut stmt = self.conn.prepare(
            "SELECT s.id, s.slug, s.title, s.url, s.cover_url, s.is_airing, s.followed,
                    s.next_episode_at, s.site_episode_count, s.watched_externally, s.kind,
                    COUNT(e.id) AS total,
                    SUM(CASE WHEN e.seen=1 THEN 1 ELSE 0 END) AS seen,
                    MAX(e.added_at) AS last_added,
                    MAX(e.seen_at) AS last_watched_at
             FROM series s
             LEFT JOIN episodes e ON e.series_id = s.id
             WHERE s.source_id=?1 AND (s.followed=1 OR s.watched_externally=1)
             GROUP BY s.id
             ORDER BY s.title",
        )?;
        let rows = stmt
            .query_map([source_id], |r| {
                let series = Self::row_to_series(r)?;
                Ok((
                    series,
                    r.get::<_, i64>("watched_externally")? != 0,
                    r.get::<_, Option<String>>("kind")?,
                    r.get::<_, i64>("total")?,
                    r.get::<_, Option<i64>>("seen")?.unwrap_or(0),
                    r.get::<_, Option<String>>("last_added")?,
                    r.get::<_, Option<String>>("last_watched_at")?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        // Bulk-fetch genres for every series in this result set in one extra
        // statement (not one query per series — see the library-filters
        // design doc's N+1 warning). `rusqlite` has no native array-bind, so
        // the IN(...) list is built from `?N` placeholders.
        let ids: Vec<i64> = rows.iter().map(|(series, ..)| series.id).collect();
        let mut genres_by_series: std::collections::HashMap<i64, Vec<String>> =
            std::collections::HashMap::new();
        if !ids.is_empty() {
            let placeholders = vec!["?"; ids.len()].join(",");
            let sql = format!(
                "SELECT series_id, genre FROM series_genres WHERE series_id IN ({}) ORDER BY genre",
                placeholders
            );
            let mut gstmt = self.conn.prepare(&sql)?;
            let params = rusqlite::params_from_iter(ids.iter());
            let grows = gstmt
                .query_map(params, |r| {
                    Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            for (series_id, genre) in grows {
                genres_by_series.entry(series_id).or_default().push(genre);
            }
        }

        let mut out = Vec::with_capacity(rows.len());
        for (series, watched_externally, kind, total_episodes, seen_episodes, last_added, last_watched_at) in rows {
            let next_episode = self.next_unseen_episode(series.id)?;
            let genres = genres_by_series.remove(&series.id).unwrap_or_default();
            out.push(crate::models::LibraryItem {
                series,
                total_episodes,
                seen_episodes,
                last_added,
                next_episode,
                last_watched_at,
                watched_externally,
                kind,
                genres,
            });
        }
        Ok(out)
    }


    /// Series with the given `backlog_status` ('want' or 'discarded'), for
    /// the swipe mode's "Listas" sub-view.
    pub fn list_backlog(&self, source_id: i64, status: &str) -> Result<Vec<crate::models::Series>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, slug, title, url, cover_url, is_airing, followed, next_episode_at, site_episode_count
             FROM series WHERE source_id=?1 AND backlog_status=?2 ORDER BY title",
        )?;
        let rows = stmt
            .query_map((source_id, status), Self::row_to_series)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Series with `watched_externally=1` ("Ya lo vi" catalog swipe), for
    /// the Listas view's "Ya vistas" sub-list — mirrors `list_backlog`.
    pub fn list_watched_externally(&self, source_id: i64) -> Result<Vec<crate::models::Series>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, slug, title, url, cover_url, is_airing, followed, next_episode_at, site_episode_count
             FROM series WHERE source_id=?1 AND watched_externally=1 ORDER BY title",
        )?;
        let rows = stmt
            .query_map([source_id], Self::row_to_series)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Hard-delete a series and everything referencing it (episodes,
    /// genres). No `ON DELETE CASCADE` is declared on the schema, so this
    /// deletes children first. Safe to call on any series — used both by
    /// swipe undo (fresh row, nothing else references it yet) and "Eliminar
    /// del todo" on discarded backlog rows (which never have episodes).
    pub fn delete_series(&self, series_id: i64) -> Result<()> {
        self.conn.execute("DELETE FROM episodes WHERE series_id=?1", [series_id])?;
        self.conn.execute("DELETE FROM series_genres WHERE series_id=?1", [series_id])?;
        self.conn.execute("DELETE FROM series WHERE id=?1", [series_id])?;
        Ok(())
    }

    pub fn list_followed(&self, source_id: i64) -> Result<Vec<crate::models::Series>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, slug, title, url, cover_url, is_airing, followed, next_episode_at, site_episode_count
             FROM series WHERE source_id=?1 AND followed=1 ORDER BY title",
        )?;
        let rows = stmt
            .query_map([source_id], Self::row_to_series)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
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





    /// The subset of a `series` row's fields the swipe-history strip needs
    /// to render one entry (`commands::list_swipe_history`): title, poster,
    /// url (so the frontend can clear it from its decided-set on undo /
    /// return-to-deck), and the two decision signals a catalog-swipe row can
    /// carry (`backlog_status`, `watched_externally` — catalog rows are
    /// never `followed` by `decide_catalog_card`, so that flag isn't needed
    /// here). `None` when the row no longer exists (already deleted by an earlier
    /// undo/return-to-deck) — the caller skips it rather than erroring, so
    /// the history strip self-heals.
    pub fn get_series_for_history(&self, series_id: i64) -> Result<Option<SwipeHistoryRow>> {
        Ok(self
            .conn
            .query_row(
                "SELECT title, cover_url, backlog_status, watched_externally, url FROM series WHERE id=?1",
                [series_id],
                |r| {
                    Ok(SwipeHistoryRow {
                        title: r.get(0)?,
                        poster_url: r.get(1)?,
                        backlog_status: r.get(2)?,
                        watched_externally: r.get::<_, i64>(3)? != 0,
                        url: r.get(4)?,
                    })
                },
            )
            .ok())
    }
}

mod sources;
mod settings;
mod episodes;
mod catalog;
mod stats;
mod airing;

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
    fn upsert_series_writes_and_updates_scan_owned_airing_metadata() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
        let mut s = mk_airing("x", "X", Some(1_783_350_140));
        s.site_episode_count = Some(2);
        let sid = db.upsert_series(src, &s).unwrap();
        let got = db.list_airing(src).unwrap().into_iter().find(|r| r.id == sid).unwrap();
        assert_eq!(got.next_episode_at, Some(1_783_350_140));
        assert_eq!(got.site_episode_count, Some(2));

        // A re-scan with fresh values overwrites both (scan-owned fields).
        let mut s2 = mk_airing("x", "X", Some(1_783_954_940));
        s2.site_episode_count = Some(3);
        db.upsert_series(src, &s2).unwrap();
        let got = db.list_airing(src).unwrap().into_iter().find(|r| r.id == sid).unwrap();
        assert_eq!(got.next_episode_at, Some(1_783_954_940));
        assert_eq!(got.site_episode_count, Some(3));
    }

    // ---- Cross-site follow carry-over (2026-07-12) ----

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
    fn carry_follow_only_touches_unfollowed_rows() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("A", "a", "animeytx").unwrap();

        let sid = db.upsert_series(src, &mk_airing("s", "S", None)).unwrap();
        assert!(db.carry_follow(sid, 4).unwrap(), "unfollowed row carries");
        assert_eq!(db.take_carried_seen_number(sid).unwrap(), Some(4));
        // Applied once: a second take yields None.
        assert_eq!(db.take_carried_seen_number(sid).unwrap(), None);

        // Already-followed row: carry is a no-op and leaves no watermark.
        let sid2 = db.upsert_series(src, &mk_airing("t", "T", None)).unwrap();
        db.set_followed(sid2, true).unwrap();
        assert!(!db.carry_follow(sid2, 9).unwrap(), "followed row is left untouched");
        assert_eq!(db.take_carried_seen_number(sid2).unwrap(), None);
    }

    #[test]
    fn carried_watermark_marks_episodes_seen_via_cascade() {
        // Simulates refresh()'s apply-once step: carry a follow, later fetch
        // the new site's episodes, then apply the stored watermark.
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("B", "b", "tioanime").unwrap();
        let sid = db.upsert_series(src, &mk_airing("x", "X", None)).unwrap();

        db.carry_follow(sid, 3).unwrap();
        // Episodes arrive on the new site (all unseen initially).
        insert_eps_seen_up_to(&db, sid, 6, 0);

        // refresh() step: read+clear the watermark, cascade-mark up to it.
        let n = db.take_carried_seen_number(sid).unwrap().expect("watermark present");
        db.set_seen_cascade(sid, &n.to_string(), true).unwrap();

        let eps = db.list_series_episodes(sid).unwrap();
        let seen: Vec<&str> = eps.iter().filter(|e| e.seen).map(|e| e.number.as_str()).collect();
        assert_eq!(seen, vec!["1", "2", "3"], "episodes 1..=3 seen, 4..=6 unseen");
        // Applied once — column cleared.
        assert_eq!(db.take_carried_seen_number(sid).unwrap(), None);
    }

    // ---- Stats clarity: franchise_key + distinct_anime (2026-07-12) ----

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

    // ---- Episode/anime counts fix: real data preferred over catalog estimate,
    // no double counting (2026-07-14) ----

    #[test]
    fn unfollowed_watched_externally_series_with_real_seen_episodes_counts_as_real_not_catalog() {
        // followed=0, watched_externally=1, 3 real seen episodes, but ALSO
        // linked to a catalog row with 12 episodes. The real data must win:
        // the 3 seen episodes count in episodes_watched, and the catalog's
        // 12 episodes must NOT also count in episodes_watched_external.
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("A", "a", "animeytx").unwrap();

        db.upsert_catalog_anime(
            &crate::anilist::CatalogAnime {
                id: 300, title: "Ext".into(), title_romaji: None, title_english: None,
                cover_url: None, format: Some("TV".into()), genres: vec![],
                episodes: Some(12), average_score: None, popularity: None,
                url: "https://anilist.co/anime/300".into(),
            },
            0,
        ).unwrap();
        let sid = db.upsert_series(src, &mk_airing("ext1", "Ext", None)).unwrap();
        db.set_watched_externally(sid, true).unwrap();
        db.set_anilist_id(sid, 300).unwrap();
        db.set_kind(sid, "TV").unwrap();
        for n in 1..=3 {
            db.insert_episode(&crate::models::Episode {
                id: 0, series_id: sid, number: n.to_string(), title: None,
                url: format!("https://site/anime/ext1-capitulo-{n}/"),
                released_at: None, seen: true,
            }).unwrap();
        }

        let summary = db.get_watch_summary(src).unwrap();
        assert_eq!(summary.episodes_watched, 3, "the 3 real seen episodes count, even though the series isn't followed");
        assert_eq!(summary.episodes_watched_external, 0, "catalog estimate suppressed — real data exists for this series");

        let insights = db.get_watch_insights(src).unwrap();
        assert_eq!(insights.estimated_minutes_tracked, 72, "3 seen eps * 24 min (TV), tracked minutes aren't followed-only anymore");
        assert_eq!(insights.estimated_minutes_external, 0, "no catalog minutes — real seen episodes take precedence");
    }

    #[test]
    fn watched_externally_series_without_episodes_counts_via_catalog_estimate() {
        // watched_externally=1, no scraped episodes at all: falls back to the
        // catalog's episode count for both the episode count and the minutes.
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("A", "a", "animeytx").unwrap();

        db.upsert_catalog_anime(
            &crate::anilist::CatalogAnime {
                id: 301, title: "Ext2".into(), title_romaji: None, title_english: None,
                cover_url: None, format: Some("TV".into()), genres: vec![],
                episodes: Some(12), average_score: None, popularity: None,
                url: "https://anilist.co/anime/301".into(),
            },
            0,
        ).unwrap();
        let sid = db.upsert_series(src, &mk_airing("ext2", "Ext2", None)).unwrap();
        db.set_watched_externally(sid, true).unwrap();
        db.set_anilist_id(sid, 301).unwrap();

        let summary = db.get_watch_summary(src).unwrap();
        assert_eq!(summary.episodes_watched, 0, "no real episodes for this series");
        assert_eq!(summary.episodes_watched_external, 12, "falls back to the catalog's episode count");

        let insights = db.get_watch_insights(src).unwrap();
        assert_eq!(insights.estimated_minutes_external, 12 * 24, "12 catalog eps * minutes_per_episode(TV)");
    }

    #[test]
    fn followed_series_with_no_seen_episodes_excluded_from_distinct_anime() {
        // followed=1 with zero seen episodes must NOT count toward
        // distinct_anime — it has no watch evidence. A watched-externally
        // series (with seen evidence) still counts.
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("A", "a", "animeytx").unwrap();

        // Followed, zero episodes seen — must be excluded.
        let unseen_id = db.upsert_series(src, &mk_airing("unseen1", "Unseen Show", None)).unwrap();
        db.set_followed(unseen_id, true).unwrap();
        db.insert_episode(&crate::models::Episode {
            id: 0, series_id: unseen_id, number: "1".into(), title: None,
            url: "https://site/anime/unseen1-capitulo-1/".into(),
            released_at: None, seen: false,
        }).unwrap();

        // Watched-externally, no scraped episodes — has watch evidence via
        // the flag itself, must count.
        let ext_id = db.upsert_series(src, &mk_airing("ext3", "Ext3", None)).unwrap();
        db.set_watched_externally(ext_id, true).unwrap();

        let summary = db.get_watch_summary(src).unwrap();
        assert_eq!(summary.distinct_anime, 1, "only the watched-externally show counts; the unwatched followed show is excluded");
    }

    #[test]
    fn followed_and_watched_externally_series_with_seen_episodes_counts_once_in_hours() {
        // followed=1 AND watched_externally=1, linked to a catalog row, with
        // real seen episodes: must contribute minutes exactly once (from the
        // real data), not twice (real + catalog estimate).
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("A", "a", "animeytx").unwrap();

        db.upsert_catalog_anime(
            &crate::anilist::CatalogAnime {
                id: 302, title: "Both".into(), title_romaji: None, title_english: None,
                cover_url: None, format: Some("TV".into()), genres: vec![],
                episodes: Some(12), average_score: None, popularity: None,
                url: "https://anilist.co/anime/302".into(),
            },
            0,
        ).unwrap();
        let sid = db.upsert_series(src, &mk_airing("both1", "Both", None)).unwrap();
        db.set_followed(sid, true).unwrap();
        db.set_watched_externally(sid, true).unwrap();
        db.set_anilist_id(sid, 302).unwrap();
        db.set_kind(sid, "TV").unwrap();
        for n in 1..=3 {
            db.insert_episode(&crate::models::Episode {
                id: 0, series_id: sid, number: n.to_string(), title: None,
                url: format!("https://site/anime/both1-capitulo-{n}/"),
                released_at: None, seen: true,
            }).unwrap();
        }

        let insights = db.get_watch_insights(src).unwrap();
        assert_eq!(insights.estimated_minutes_tracked, 72, "3 seen eps * 24 min (TV)");
        assert_eq!(insights.estimated_minutes_external, 0, "no double count — the catalog estimate is suppressed for this series");

        // Criterion 4: episodes and hours cover the exact same universe —
        // episodes_watched + episodes_watched_external episode total matches
        // what the minutes were computed from (3 real eps, 0 catalog eps).
        let summary = db.get_watch_summary(src).unwrap();
        assert_eq!(summary.episodes_watched, 3);
        assert_eq!(summary.episodes_watched_external, 0);
    }

    #[test]
    fn series_genres_insert_is_idempotent_and_lists_sorted() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
        let s = crate::models::Series {
            id: 0, slug: "x".into(), title: "X".into(),
            url: "u".into(), cover_url: None, is_airing: false, followed: false, next_episode_at: None, site_episode_count: None,
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
        let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
        let s = crate::models::Series {
            id: 0, slug: "x".into(), title: "X".into(),
            url: "u".into(), cover_url: None, is_airing: false, followed: false, next_episode_at: None, site_episode_count: None,
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
    fn list_backlog_filters_by_status() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
        let want = crate::models::Series {
            id: 0, slug: "want".into(), title: "Want".into(),
            url: "u1".into(), cover_url: None, is_airing: false, followed: false, next_episode_at: None, site_episode_count: None,
        };
        let sid_want = db.upsert_series(src, &want).unwrap();
        db.set_backlog_status(sid_want, Some("want")).unwrap();

        let discarded = crate::models::Series {
            id: 0, slug: "disc".into(), title: "Disc".into(),
            url: "u2".into(), cover_url: None, is_airing: false, followed: false, next_episode_at: None, site_episode_count: None,
        };
        let sid_disc = db.upsert_series(src, &discarded).unwrap();
        db.set_backlog_status(sid_disc, Some("discarded")).unwrap();

        let wants = db.list_backlog(src, "want").unwrap();
        assert_eq!(wants.len(), 1);
        assert_eq!(wants[0].id, sid_want);

        let discards = db.list_backlog(src, "discarded").unwrap();
        assert_eq!(discards.len(), 1);
        assert_eq!(discards[0].id, sid_disc);
    }

    #[test]
    fn list_watched_externally_filters_by_flag() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();

        let watched = crate::models::Series {
            id: 0, slug: "watched".into(), title: "Watched".into(),
            url: "u1".into(), cover_url: None, is_airing: false, followed: false, next_episode_at: None, site_episode_count: None,
        };
        let sid_watched = db.upsert_series(src, &watched).unwrap();
        db.set_watched_externally(sid_watched, true).unwrap();

        let other = crate::models::Series {
            id: 0, slug: "other".into(), title: "Other".into(),
            url: "u2".into(), cover_url: None, is_airing: false, followed: false, next_episode_at: None, site_episode_count: None,
        };
        let sid_other = db.upsert_series(src, &other).unwrap();
        db.set_backlog_status(sid_other, Some("want")).unwrap();

        let watched_rows = db.list_watched_externally(src).unwrap();
        assert_eq!(watched_rows.len(), 1);
        assert_eq!(watched_rows[0].id, sid_watched);
    }

    /// Root-cause test for the "Ya lo vi" Library-visibility bug (see
    /// docs/superpowers/specs/2026-07-12-ya-lo-vi-library-visibility-design.md):
    /// `list_library` used to filter on `followed=1` only, silently excluding
    /// every `watched_externally=1, followed=0` catalog "Seen" row. Both a
    /// normal followed series (with episodes) and a watched-externally-only
    /// series (zero episodes, since AniList catalog Seen never scrapes) must
    /// come back.
    #[test]
    fn list_library_includes_watched_externally_rows_with_no_episodes() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();

        // Followed series with episodes — the existing, already-working path.
        let followed = crate::models::Series {
            id: 0, slug: "followed".into(), title: "Followed".into(),
            url: "u1".into(), cover_url: None, is_airing: true, followed: false, next_episode_at: None, site_episode_count: None,
        };
        let sid_followed = db.upsert_series(src, &followed).unwrap();
        db.set_followed(sid_followed, true).unwrap();
        db.insert_episode(&crate::models::Episode {
            id: 0, series_id: sid_followed, number: "1".into(), title: None,
            url: "ep1".into(), released_at: None, seen: true,
        }).unwrap();

        // "Ya lo vi" catalog row: watched_externally=1, followed=0, no episodes.
        let watched = crate::models::Series {
            id: 0, slug: "watched".into(), title: "Watched".into(),
            url: "u2".into(), cover_url: None, is_airing: false, followed: false, next_episode_at: None, site_episode_count: None,
        };
        let sid_watched = db.upsert_series(src, &watched).unwrap();
        db.set_watched_externally(sid_watched, true).unwrap();

        let items = db.list_library(src).unwrap();
        assert_eq!(items.len(), 2, "both the followed and the watched-externally-only rows must be returned");

        let watched_item = items.iter().find(|it| it.series.id == sid_watched)
            .expect("watched_externally=1, followed=0 row must be present in list_library");
        assert!(watched_item.watched_externally);
        assert_eq!(watched_item.total_episodes, 0);

        let followed_item = items.iter().find(|it| it.series.id == sid_followed).unwrap();
        assert!(!followed_item.watched_externally);
        assert_eq!(followed_item.total_episodes, 1);
    }

    #[test]
    fn list_library_returns_kind_and_genres_via_one_bulk_query() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();

        let s1 = crate::models::Series {
            id: 0, slug: "s1".into(), title: "S1".into(),
            url: "u1".into(), cover_url: None, is_airing: false, followed: false, next_episode_at: None, site_episode_count: None,
        };
        let sid1 = db.upsert_series(src, &s1).unwrap();
        db.set_followed(sid1, true).unwrap();
        db.set_kind(sid1, "TV").unwrap();
        db.insert_series_genres(sid1, &["Accion".to_string(), "Comedia".to_string()]).unwrap();

        // A second series with no kind/genres set at all — must come back
        // with kind=None and an empty genres vec, not an error or a missing row.
        let s2 = crate::models::Series {
            id: 0, slug: "s2".into(), title: "S2".into(),
            url: "u2".into(), cover_url: None, is_airing: false, followed: false, next_episode_at: None, site_episode_count: None,
        };
        let sid2 = db.upsert_series(src, &s2).unwrap();
        db.set_followed(sid2, true).unwrap();

        let items = db.list_library(src).unwrap();
        let item1 = items.iter().find(|it| it.series.id == sid1).unwrap();
        assert_eq!(item1.kind.as_deref(), Some("TV"));
        assert_eq!(item1.genres, vec!["Accion".to_string(), "Comedia".to_string()]);

        let item2 = items.iter().find(|it| it.series.id == sid2).unwrap();
        assert_eq!(item2.kind, None);
        assert!(item2.genres.is_empty());
    }

    #[test]
    fn delete_series_cascades_episodes_and_genres() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
        let s = crate::models::Series {
            id: 0, slug: "x".into(), title: "X".into(),
            url: "u".into(), cover_url: None, is_airing: false, followed: true, next_episode_at: None, site_episode_count: None,
        };
        let sid = db.upsert_series(src, &s).unwrap();
        db.insert_series_genres(sid, &["Seinen".to_string()]).unwrap();
        db.insert_episode(&crate::models::Episode {
            id: 0, series_id: sid, number: "1".into(), title: None,
            url: "e1".into(), released_at: None, seen: false,
        }).unwrap();

        db.delete_series(sid).unwrap();

        assert!(db.list_followed(src).unwrap().iter().all(|f| f.id != sid));
        assert!(db.list_series_genres(sid).unwrap().is_empty());
        assert!(db.list_series_episodes(sid).unwrap().is_empty());
    }

    #[test]
    fn get_series_for_link_reads_link_relevant_fields() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
        let s = crate::models::Series {
            id: 0, slug: "anilist-42".into(), title: "X".into(),
            url: "https://anilist.co/anime/42".into(), cover_url: None, is_airing: false, followed: false, next_episode_at: None, site_episode_count: None,
        };
        let sid = db.upsert_series(src, &s).unwrap();
        db.set_anilist_id(sid, 42).unwrap();
        db.set_backlog_status(sid, Some("want")).unwrap();

        let info = db.get_series_for_link(sid).unwrap().unwrap();
        assert_eq!(info.id, sid);
        assert_eq!(info.source_id, src);
        assert_eq!(info.slug, "anilist-42");
        assert_eq!(info.anilist_id, Some(42));
        assert_eq!(info.backlog_status.as_deref(), Some("want"));
        assert!(!info.followed);
        assert!(!info.watched_externally);
    }

    #[test]
    fn get_series_for_link_none_for_missing_row() {
        let db = Db::open(":memory:").unwrap();
        assert!(db.get_series_for_link(999).unwrap().is_none());
    }

    /// Pins the idempotence fix: a row must be treated as "already linked"
    /// (no re-scrape) both when it never had a catalog origin and when a
    /// previous link already rewrote its slug onto the real site, even
    /// though `anilist_id` itself is never cleared by a successful link.
    #[test]
    fn already_linked_to_site_covers_both_null_anilist_id_and_relinked_slug() {
        let plain_site_row = SeriesForLink {
            id: 1, source_id: 1, slug: "baki-dou".into(), anilist_id: None,
            followed: true, backlog_status: None, watched_externally: false,
        };
        assert!(plain_site_row.already_linked_to_site());

        let unlinked_catalog_row = SeriesForLink {
            id: 2, source_id: 1, slug: "anilist-42".into(), anilist_id: Some(42),
            followed: false, backlog_status: Some("want".into()), watched_externally: false,
        };
        assert!(!unlinked_catalog_row.already_linked_to_site());

        // The critical case: a catalog row that was already linked in a
        // previous call keeps its `anilist_id` (relink_series never clears
        // it) but its slug is now the real site slug — must still count as
        // already-linked, or a second call (e.g. a race between the Seen
        // swipe's fire-and-forget link and opening SeriesDetail) would
        // re-search and re-scrape.
        let already_relinked_row = SeriesForLink {
            id: 3, source_id: 1, slug: "baki-dou".into(), anilist_id: Some(42),
            followed: false, backlog_status: None, watched_externally: true,
        };
        assert!(already_relinked_row.already_linked_to_site());
    }

    #[test]
    fn find_series_id_by_slug_excludes_given_id_and_scopes_by_source() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
        let real = crate::models::Series {
            id: 0, slug: "baki-dou".into(), title: "Baki-dou".into(),
            url: "u1".into(), cover_url: None, is_airing: true, followed: false, next_episode_at: None, site_episode_count: None,
        };
        let real_id = db.upsert_series(src, &real).unwrap();
        let synthetic = crate::models::Series {
            id: 0, slug: "anilist-7".into(), title: "Baki-dou".into(),
            url: "https://anilist.co/anime/7".into(), cover_url: None, is_airing: false, followed: false, next_episode_at: None, site_episode_count: None,
        };
        let synthetic_id = db.upsert_series(src, &synthetic).unwrap();

        // Excluding the synthetic row's own id, "baki-dou" resolves to the real row.
        assert_eq!(
            db.find_series_id_by_slug(src, "baki-dou", synthetic_id).unwrap(),
            Some(real_id)
        );
        // No collision for a slug nothing owns.
        assert_eq!(db.find_series_id_by_slug(src, "no-such-slug", synthetic_id).unwrap(), None);
    }

    /// Acceptance criterion: "linking a synthetic row onto an existing site
    /// slug merges rather than violating uniqueness" — `merge_series_into`
    /// must move the synthetic row's decision flags onto the existing real
    /// row (OR'd / most-specific-wins) and delete the synthetic row, never
    /// attempting an insert/update that could hit `UNIQUE(source_id, slug)`.
    #[test]
    fn merge_series_into_transfers_flags_and_deletes_synthetic() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
        let existing = crate::models::Series {
            id: 0, slug: "baki-dou".into(), title: "Baki-dou".into(),
            url: "https://site/tv/baki-dou/".into(), cover_url: None, is_airing: true, followed: false, next_episode_at: None, site_episode_count: None,
        };
        let existing_id = db.upsert_series(src, &existing).unwrap();

        let synthetic = crate::models::Series {
            id: 0, slug: "anilist-7".into(), title: "Baki-dou".into(),
            url: "https://anilist.co/anime/7".into(), cover_url: None, is_airing: false, followed: false, next_episode_at: None, site_episode_count: None,
        };
        let synthetic_id = db.upsert_series(src, &synthetic).unwrap();
        db.set_anilist_id(synthetic_id, 7).unwrap();
        db.set_followed(synthetic_id, true).unwrap();
        db.set_backlog_status(synthetic_id, Some("want")).unwrap();

        db.merge_series_into(existing_id, synthetic_id).unwrap();

        // Synthetic row is gone.
        assert!(db.get_series_for_link(synthetic_id).unwrap().is_none());
        // Existing row picked up the synthetic row's flags.
        let merged = db.get_series_for_link(existing_id).unwrap().unwrap();
        assert!(merged.followed);
        assert_eq!(merged.backlog_status.as_deref(), Some("want"));
        // Existing row's own slug/url survive untouched (it stays canonical).
        assert_eq!(merged.slug, "baki-dou");
    }

    #[test]
    fn merge_series_into_keeps_existing_backlog_status_when_it_already_has_one() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
        let existing = crate::models::Series {
            id: 0, slug: "baki-dou".into(), title: "Baki-dou".into(),
            url: "https://site/tv/baki-dou/".into(), cover_url: None, is_airing: true, followed: false, next_episode_at: None, site_episode_count: None,
        };
        let existing_id = db.upsert_series(src, &existing).unwrap();
        db.set_backlog_status(existing_id, Some("discarded")).unwrap();

        let synthetic = crate::models::Series {
            id: 0, slug: "anilist-7".into(), title: "Baki-dou".into(),
            url: "https://anilist.co/anime/7".into(), cover_url: None, is_airing: false, followed: false, next_episode_at: None, site_episode_count: None,
        };
        let synthetic_id = db.upsert_series(src, &synthetic).unwrap();
        db.set_backlog_status(synthetic_id, Some("want")).unwrap();

        db.merge_series_into(existing_id, synthetic_id).unwrap();

        // Existing row's own (more specific/pre-existing) status wins.
        let merged = db.get_series_for_link(existing_id).unwrap().unwrap();
        assert_eq!(merged.backlog_status.as_deref(), Some("discarded"));
    }

    #[test]
    fn relink_series_updates_slug_url_cover_and_kind_in_place() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
        let synthetic = crate::models::Series {
            id: 0, slug: "anilist-7".into(), title: "Baki-dou".into(),
            url: "https://anilist.co/anime/7".into(), cover_url: Some("https://anilist-cdn/7.jpg".into()),
            is_airing: false, followed: true, next_episode_at: None, site_episode_count: None,
        };
        let sid = db.upsert_series(src, &synthetic).unwrap();
        db.set_anilist_id(sid, 7).unwrap();
        // upsert_series deliberately never writes `followed` (see its own
        // doc comment) — set it the same way the rest of the codebase does.
        db.set_followed(sid, true).unwrap();

        db.relink_series(sid, "baki-dou", "https://site/tv/baki-dou/", Some("https://site/cover.jpg"), "TV").unwrap();

        let url = db.get_series_url(sid).unwrap().unwrap();
        assert_eq!(url, "https://site/tv/baki-dou/");
        // followed/anilist_id survive — relink_series only touches slug/url/cover/kind.
        let info = db.get_series_for_link(sid).unwrap().unwrap();
        assert_eq!(info.slug, "baki-dou");
        assert!(info.followed);
        assert_eq!(info.anilist_id, Some(7));
    }

    #[test]
    fn replace_series_genres_drops_old_genres_instead_of_accumulating() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
        let s = crate::models::Series {
            id: 0, slug: "x".into(), title: "X".into(),
            url: "u".into(), cover_url: None, is_airing: false, followed: false, next_episode_at: None, site_episode_count: None,
        };
        let sid = db.upsert_series(src, &s).unwrap();
        db.insert_series_genres(sid, &["Action".to_string(), "Isekai".to_string()]).unwrap();

        db.replace_series_genres(sid, &["Drama".to_string()]).unwrap();

        assert_eq!(db.list_series_genres(sid).unwrap(), vec!["Drama".to_string()]);
    }


    #[test]
    fn series_needs_genre_backfill_reflects_series_genres_rows() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
        let s = crate::models::Series {
            id: 0, slug: "x".into(), title: "X".into(),
            url: "u".into(), cover_url: None, is_airing: false, followed: true, next_episode_at: None, site_episode_count: None,
        };
        let sid = db.upsert_series(src, &s).unwrap();

        assert!(db.series_needs_genre_backfill(sid).unwrap());
        db.insert_series_genres(sid, &["Seinen".to_string()]).unwrap();
        assert!(!db.series_needs_genre_backfill(sid).unwrap());
    }

    #[test]
    fn engaged_series_titles_covers_followed_want_discarded_and_watched_externally() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();

        let make = |slug: &str, title: &str| crate::models::Series {
            id: 0,
            slug: slug.into(),
            title: title.into(),
            url: format!("https://example.com/tv/{slug}"),
            cover_url: None,
            is_airing: false,
            followed: false,
            next_episode_at: None,
            site_episode_count: None,
        };

        let sid_followed = db.upsert_series(src, &make("followed-show", "Followed Show")).unwrap();
        db.set_followed(sid_followed, true).unwrap();

        let sid_want = db.upsert_series(src, &make("want-show", "Want Show")).unwrap();
        db.set_backlog_status(sid_want, Some("want")).unwrap();

        let sid_discarded = db.upsert_series(src, &make("discarded-show", "Discarded Show")).unwrap();
        db.set_backlog_status(sid_discarded, Some("discarded")).unwrap();

        let sid_watched = db.upsert_series(src, &make("watched-show", "Watched Show")).unwrap();
        db.set_watched_externally(sid_watched, true).unwrap();

        // Untouched row must NOT be engaged.
        db.upsert_series(src, &make("untouched-show", "Untouched Show")).unwrap();

        let titles: std::collections::HashSet<String> = db.engaged_series_titles(src).unwrap().into_iter().collect();
        assert!(titles.contains("Followed Show"));
        assert!(titles.contains("Want Show"));
        assert!(titles.contains("Discarded Show"));
        assert!(titles.contains("Watched Show"));
        assert!(!titles.contains("Untouched Show"));
    }

    #[test]
    fn get_series_for_history_returns_none_for_a_deleted_row() {
        let db = Db::open(":memory:").unwrap();
        assert!(db.get_series_for_history(999).unwrap().is_none());
    }

    /// The history strip's client-side reappearance fix needs the row's
    /// `url` to clear the swiped card from the frontend's decided-set when
    /// the card legitimately returns to the deck (undo / return-to-deck) —
    /// see `SwipeHistoryItem::url` and the design spec's Fix 1.
    #[test]
    fn get_series_for_history_includes_the_rows_url() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
        let s = crate::models::Series {
            id: 0, slug: "anilist-7".into(), title: "T".into(),
            url: "https://anilist.co/anime/7/t".into(), cover_url: None,
            is_airing: false, followed: false, next_episode_at: None, site_episode_count: None,
        };
        let sid = db.upsert_series(src, &s).unwrap();
        let row = db.get_series_for_history(sid).unwrap().unwrap();
        assert_eq!(row.url, "https://anilist.co/anime/7/t");
    }

    #[test]
    fn anilist_id_and_watched_externally_round_trip() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
        let s = crate::models::Series {
            id: 0, slug: "anilist-42".into(), title: "X".into(),
            url: "u".into(), cover_url: None, is_airing: false, followed: false, next_episode_at: None, site_episode_count: None,
        };
        let sid = db.upsert_series(src, &s).unwrap();

        db.set_anilist_id(sid, 42).unwrap();
        db.set_watched_externally(sid, true).unwrap();

        let (anilist_id, watched): (Option<i64>, i64) = db.conn.query_row(
            "SELECT anilist_id, watched_externally FROM series WHERE id=?1",
            [sid],
            |r| Ok((r.get(0)?, r.get(1)?)),
        ).unwrap();
        assert_eq!(anilist_id, Some(42));
        assert_eq!(watched, 1);
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
