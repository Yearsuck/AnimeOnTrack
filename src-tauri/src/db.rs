use anyhow::Result;
use rusqlite::types::Value;
use rusqlite::Connection;
use serde::Deserialize;

/// Filters for browsing the locally-synced AniList catalog (`Catalog.tsx`'s
/// search/filter bar). All fields are optional/empty-by-default so
/// `CatalogFilter::default()` is a no-op filter — see
/// `list_catalog`/`catalog_count`, which are thin wrappers over the
/// `_filtered` versions with the default filter, so unfiltered behavior is
/// unchanged.
#[derive(Debug, Default, Clone, Deserialize)]
#[serde(default)]
pub struct CatalogFilter {
    /// Title substring match, case-insensitive (`LIKE ... COLLATE NOCASE`).
    pub search: Option<String>,
    /// AND semantics — a matching title must carry every listed genre.
    pub genres: Vec<String>,
    /// Exact match against `anilist_catalog.format` (e.g. "TV", "MOVIE").
    pub format: Option<String>,
    /// Inclusive floor on `average_score` (0-100 scale).
    pub min_score: Option<i64>,
    /// Episode-count bucket: "1" | "2-12" | "13-26" | "27+" | "unknown"
    /// (`unknown` = `episodes IS NULL`). Unrecognized values are ignored.
    pub episodes: Option<String>,
}

/// Ordering for the pending queue, by how many episodes each series still
/// has left to watch. `RemainingAsc` = fewest-left first (quick wins).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PendingSort {
    RemainingAsc,
    RemainingDesc,
}

/// The subset of a `series` row's fields the swipe-history strip needs —
/// see `Db::get_series_for_history` and `commands::list_swipe_history`.
#[derive(Debug, Clone, PartialEq)]
pub struct SwipeHistoryRow {
    pub title: String,
    pub poster_url: Option<String>,
    pub backlog_status: Option<String>,
    pub watched_externally: bool,
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

/// Extract a comparable numeric value from an episode-number string, e.g.
/// "12" -> 12.0, "12.5" -> 12.5 (OVA/special numbering), "1x05" -> season 1
/// episode 5 packed as 100005.0 (season-prefixed numbering seen on
/// multi-cour series pages) so cascade comparisons order by (season,
/// episode) instead of just the season digit — packing just the season
/// would make every episode in the same season parse identically, and
/// un-marking one used to wipe every other episode in that season too.
/// Returns `None` when there are no leading digits at all (e.g. "OVA"), so
/// callers can fall back to an exact-string match instead of guessing an
/// ordering.
fn parse_ep_number(s: &str) -> Option<f64> {
    let trimmed = s.trim();
    if let Some((season, ep)) = trimmed.split_once(['x', 'X']) {
        if let (Ok(season), Ok(ep)) = (season.trim().parse::<f64>(), ep.trim().parse::<f64>()) {
            return Some(season * 100_000.0 + ep);
        }
    }
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

    /// `(title, title_romaji, title_english)` for a synced catalog entry —
    /// `link_catalog_series` tries `title_romaji.unwrap_or(title)` first,
    /// then `title_english` if that fails to match anything on the site.
    pub fn get_catalog_titles(&self, anilist_id: i64) -> Result<Option<(String, Option<String>, Option<String>)>> {
        Ok(self
            .conn
            .query_row(
                "SELECT title, title_romaji, title_english FROM anilist_catalog WHERE id=?1",
                [anilist_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
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

    /// Mark every episode of `series_id` seen via the existing gap-free
    /// `set_seen_cascade`, targeting whichever episode number sorts highest
    /// by `parse_ep_number` — used when a catalog title decided `Seen`
    /// (`watched_externally=1`) gets linked to a real site series and its
    /// episode list is scraped in for the first time. Reusing the cascade
    /// (rather than a blanket `UPDATE ... SET seen=1`) keeps this consistent
    /// with the same season/episode-aware ordering the rest of the app's
    /// watch-tracking relies on. A series with no episodes yet is a silent
    /// no-op, not an error.
    pub fn mark_all_episodes_seen(&self, series_id: i64) -> Result<()> {
        let mut stmt = self.conn.prepare("SELECT number FROM episodes WHERE series_id=?1")?;
        let numbers: Vec<String> = stmt
            .query_map([series_id], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let highest = numbers.iter().max_by(|a, b| {
            let av = parse_ep_number(a).unwrap_or(f64::MIN);
            let bv = parse_ep_number(b).unwrap_or(f64::MIN);
            av.partial_cmp(&bv).unwrap_or(std::cmp::Ordering::Equal)
        });
        if let Some(number) = highest {
            self.set_seen_cascade(series_id, number, true)?;
        }
        Ok(())
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

    /// Global (not per-site) user-configured genre ban list for the
    /// Descubrir catalog deck — un-prefixed `banned_genres` settings key,
    /// newline-joined like the per-site mirror list (see
    /// `commands::{load_mirrors, save_mirrors}`), but global because taste
    /// bans are a user preference, not tied to whichever site happens to be
    /// active. Additive to `EXCLUDED_CATALOG_GENRES` (Hentai/Ecchi), not a
    /// replacement for it.
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

    /// Followed-series count per genre, descending. Only covers series that
    /// have `series_genres` rows (see `refresh()`'s backfill step).
    pub fn get_genre_stats(&self, source_id: i64) -> Result<Vec<crate::models::GenreStat>> {
        let mut stmt = self.conn.prepare(
            "SELECT g.genre, COUNT(DISTINCT g.series_id) AS cnt
             FROM series_genres g
             JOIN series s ON s.id = g.series_id
             WHERE s.source_id=?1 AND s.followed=1
             GROUP BY g.genre
             ORDER BY cnt DESC, g.genre",
        )?;
        let rows = stmt
            .query_map([source_id], |r| {
                Ok(crate::models::GenreStat { genre: r.get(0)?, count: r.get(1)? })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Followed-series count per `kind` ("TV"/"OVA"/...), descending. Series
    /// with no `kind` set are excluded.
    pub fn get_type_stats(&self, source_id: i64) -> Result<Vec<crate::models::TypeStat>> {
        let mut stmt = self.conn.prepare(
            "SELECT s.kind, COUNT(*) AS cnt
             FROM series s
             WHERE s.source_id=?1 AND s.followed=1 AND s.kind IS NOT NULL AND s.kind <> ''
             GROUP BY s.kind
             ORDER BY cnt DESC, s.kind",
        )?;
        let rows = stmt
            .query_map([source_id], |r| {
                Ok(crate::models::TypeStat { kind: r.get(0)?, count: r.get(1)? })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// `true` if this series has no `series_genres` rows yet (needs a
    /// detail-page fetch to backfill). Split out as a plain sync check so the
    /// async fetch decision can be unit-tested without a scraper/AppHandle.
    pub fn series_needs_genre_backfill(&self, series_id: i64) -> Result<bool> {
        Ok(self.list_series_genres(series_id)?.is_empty())
    }

    /// Per-genre affinity score, used to weight the swipe deck's genre pick
    /// toward what this user actually likes instead of picking uniformly at
    /// random. +2 per followed series in that genre (actively watching or
    /// already watched — the strongest signal), +1 per 'want' backlog series
    /// (interested, not started yet), -1.5 per 'discarded' series (explicit
    /// pass). Genres with no signal simply don't appear in the map — the
    /// caller treats a missing entry as 0, and `weighted_pick_index` falls
    /// back to uniform whenever every candidate nets <= 0, so a new user (or
    /// a genre nobody's decided on yet) never gets a silently empty deck.
    pub fn get_genre_affinity(&self, source_id: i64) -> Result<std::collections::HashMap<String, f64>> {
        let mut stmt = self.conn.prepare(
            "SELECT sg.genre, s.followed, s.backlog_status
             FROM series_genres sg
             JOIN series s ON s.id = sg.series_id
             WHERE s.source_id=?1",
        )?;
        let rows: Vec<(String, bool, Option<String>)> = stmt
            .query_map([source_id], |r| {
                Ok((r.get(0)?, r.get::<_, i64>(1)? != 0, r.get(2)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        let mut scores: std::collections::HashMap<String, f64> = std::collections::HashMap::new();
        for (genre, followed, backlog_status) in rows {
            let delta = if followed {
                2.0
            } else {
                match backlog_status.as_deref() {
                    Some("want") => 1.0,
                    Some("discarded") => -1.5,
                    _ => 0.0,
                }
            };
            // Fold onto AniList's canonical genre name when this raw tag has
            // one (e.g. site's "Acción" and catalog's "Action" both become
            // "Action"), so the same taste signal from either source lands
            // in one bucket instead of two unrelated ones. Tags with no
            // AniList equivalent (e.g. "Isekai") keep their raw name — they
            // still carry real signal for the site path's genre pages.
            let key = crate::genres::canonical_genre(&genre)
                .map(|s| s.to_string())
                .unwrap_or(genre);
            *scores.entry(key).or_insert(0.0) += delta;
        }
        Ok(scores)
    }

    /// Followed series with their genres (aggregated) and kind, for the 3D
    /// relationship graph. One follow-up `list_series_genres` query per
    /// series — simplest option given that helper already exists, and the
    /// followed-series count here is small (never bulk-scraped).
    pub fn get_stats_graph_data(&self, source_id: i64) -> Result<Vec<crate::models::SeriesGraphNode>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, title, cover_url, kind FROM series
             WHERE source_id=?1 AND followed=1 ORDER BY title",
        )?;
        let rows: Vec<(i64, String, Option<String>, Option<String>)> = stmt
            .query_map([source_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows.into_iter()
            .map(|(id, title, cover_url, kind)| {
                Ok(crate::models::SeriesGraphNode {
                    id,
                    title,
                    cover_url,
                    genres: self.list_series_genres(id)?,
                    kind,
                })
            })
            .collect()
    }

    /// Scalar watch totals for the stats dashboard.
    pub fn get_watch_summary(&self, source_id: i64) -> Result<crate::models::WatchSummary> {
        let followed_series: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM series WHERE source_id=?1 AND followed=1",
            [source_id],
            |r| r.get(0),
        )?;
        let (episodes_watched, episodes_total): (i64, i64) = self.conn.query_row(
            "SELECT COALESCE(SUM(CASE WHEN e.seen=1 THEN 1 ELSE 0 END), 0), COUNT(e.id)
             FROM episodes e
             JOIN series s ON s.id = e.series_id
             WHERE s.source_id=?1 AND s.followed=1",
            [source_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        let backlog_want: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM series WHERE source_id=?1 AND backlog_status='want'",
            [source_id],
            |r| r.get(0),
        )?;
        Ok(crate::models::WatchSummary {
            followed_series,
            episodes_watched,
            episodes_total,
            backlog_want,
        })
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
            next_episode_at: r.get("next_episode_at")?,
            site_episode_count: r.get("site_episode_count")?,
        })
    }

    /// "En emisión" default order: most-recently-released first.
    /// `next_episode_at` is the *next* episode's release time; for a weekly
    /// series that's roughly "last release + 7 days", so the series whose
    /// episode dropped most recently has the furthest-away next episode —
    /// descending order therefore reads as newest-release-first (see the
    /// airing-sort-order design doc; non-weekly series break the inference
    /// and that's accepted). NULLs (no countdown on the card / never seen on
    /// the airing listing) sort last; `title` is a stable tie-break so
    /// equal/NULL timestamps never reorder run to run.
    pub fn list_airing(&self, source_id: i64) -> Result<Vec<crate::models::Series>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, slug, title, url, cover_url, is_airing, followed, next_episode_at, site_episode_count
             FROM series WHERE source_id=?1 AND is_airing=1
             ORDER BY next_episode_at IS NULL, next_episode_at DESC, title",
        )?;
        let rows = stmt
            .query_map([source_id], Self::row_to_series)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Followed series with their episode counts (total, seen), the most
    /// recent episode's `added_at`, and (via `next_unseen_episode`) the
    /// lowest-numbered unseen episode plus `last_watched_at`, for the
    /// library view. Series with zero scraped episodes still appear
    /// (`total=0`, `next_episode=None`) — status derivation on the frontend
    /// treats that as "plan", never dividing by zero.
    pub fn list_library(&self, source_id: i64) -> Result<Vec<crate::models::LibraryItem>> {
        let mut stmt = self.conn.prepare(
            "SELECT s.id, s.slug, s.title, s.url, s.cover_url, s.is_airing, s.followed,
                    s.next_episode_at, s.site_episode_count,
                    COUNT(e.id) AS total,
                    SUM(CASE WHEN e.seen=1 THEN 1 ELSE 0 END) AS seen,
                    MAX(e.added_at) AS last_added,
                    MAX(e.seen_at) AS last_watched_at
             FROM series s
             LEFT JOIN episodes e ON e.series_id = s.id
             WHERE s.source_id=?1 AND s.followed=1
             GROUP BY s.id
             ORDER BY s.title",
        )?;
        let rows = stmt
            .query_map([source_id], |r| {
                let series = Self::row_to_series(r)?;
                Ok((
                    series,
                    r.get::<_, i64>("total")?,
                    r.get::<_, Option<i64>>("seen")?.unwrap_or(0),
                    r.get::<_, Option<String>>("last_added")?,
                    r.get::<_, Option<String>>("last_watched_at")?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        let mut out = Vec::with_capacity(rows.len());
        for (series, total_episodes, seen_episodes, last_added, last_watched_at) in rows {
            let next_episode = self.next_unseen_episode(series.id)?;
            out.push(crate::models::LibraryItem {
                series,
                total_episodes,
                seen_episodes,
                last_added,
                next_episode,
                last_watched_at,
            });
        }
        Ok(out)
    }

    /// Lowest-numbered unseen episode of a series, or `None` if every
    /// episode is seen (or there are none) — same ordering as
    /// `list_series_episodes` (`SeriesDetail`'s query): `CAST(number AS
    /// INTEGER) ASC, id ASC`. Deliberately reuses that exact ORDER BY rather
    /// than a different numeric parse, so this can never disagree with what
    /// `SeriesDetail` shows as "next" for the same series.
    fn next_unseen_episode(&self, series_id: i64) -> Result<Option<crate::models::NextEpisode>> {
        let mut stmt = self.conn.prepare(
            "SELECT number, title, url FROM episodes
             WHERE series_id=?1 AND seen=0
             ORDER BY CAST(number AS INTEGER) ASC, id ASC
             LIMIT 1",
        )?;
        let mut rows = stmt.query_map([series_id], |r| {
            Ok(crate::models::NextEpisode {
                number: r.get(0)?,
                title: r.get(1)?,
                url: r.get(2)?,
            })
        })?;
        rows.next().transpose().map_err(Into::into)
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

    /// Episode-row count for one series — the DB side of refresh()'s skip
    /// decision (compared against the airing card's `.sb` badge).
    pub fn episode_count(&self, series_id: i64) -> Result<i64> {
        Ok(self.conn.query_row(
            "SELECT COUNT(*) FROM episodes WHERE series_id=?1",
            [series_id],
            |r| r.get(0),
        )?)
    }

    /// Highest numeric episode number we have for a series — the correct DB
    /// side of refresh()'s skip decision (see `commands::should_fetch_series`),
    /// unlike `episode_count` which is a row COUNT and gets inflated by
    /// recap/"0"/version-reupload rows (Bug B, 2026-07-12 airing-refresh fix).
    /// SQLite's `CAST(x AS INTEGER)` parses a leading integer and stops at the
    /// first non-digit, so `'13'`->13, `'1 | v2'`->1, `'0 | Recap'`->0,
    /// `'Recap'`->0. Returns 0 when the series has no episode rows.
    pub fn max_episode_number(&self, series_id: i64) -> Result<i64> {
        Ok(self.conn.query_row(
            "SELECT COALESCE(MAX(CAST(number AS INTEGER)), 0) FROM episodes WHERE series_id=?1",
            [series_id],
            |r| r.get(0),
        )?)
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
    /// `seen_at` tracks alongside: stamped `datetime('now')` when marking
    /// seen, cleared back to NULL when un-marking.
    pub fn set_seen(&self, episode_id: i64, seen: bool) -> Result<()> {
        self.conn.execute(
            "UPDATE episodes SET seen=?1, seen_at = CASE WHEN ?1=1 THEN datetime('now') ELSE NULL END
             WHERE id=?2",
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
                "UPDATE episodes SET seen=?1, seen_at = CASE WHEN ?1=1 THEN datetime('now') ELSE NULL END
                 WHERE series_id=?2 AND number=?3",
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
                // seen_at cascades along with seen — every row the cascade
                // touches gets the same stamped/cleared treatment as the
                // one the user explicitly clicked, not just that one.
                self.conn.execute(
                    "UPDATE episodes SET seen=?1, seen_at = CASE WHEN ?1=1 THEN datetime('now') ELSE NULL END
                     WHERE id=?2",
                    (seen as i64, id),
                )?;
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

    /// Unseen episodes of currently-followed series only, scoped to
    /// `source_id` — unfollowing a series must drop its episodes out of the
    /// pending count immediately, and (multi-site) a different site's
    /// followed series must never leak into this site's pending count.
    pub fn pending_count(&self, source_id: i64) -> Result<i64> {
        let n: i64 = self.conn.query_row(
            "SELECT count(*) FROM episodes e JOIN series s ON s.id = e.series_id
             WHERE e.seen=0 AND s.followed=1 AND s.source_id=?1",
            [source_id],
            |r| r.get(0),
        )?;
        Ok(n)
    }

    /// Unseen episodes of currently-followed series, joined with their
    /// series, newest first, scoped to `source_id` (see `pending_count`).
    /// Unseen episodes of currently-followed series, ordered so each series'
    /// episodes stay contiguous (grouped in the UI) and the *groups* come out
    /// sorted by how many pending episodes each series has — see `PendingSort`.
    /// `COUNT(*) OVER (PARTITION BY s.id)` is the per-series remaining count;
    /// `s.title` then `e.added_at DESC` keep ordering stable within a group and
    /// across equal-count series.
    pub fn list_pending(
        &self,
        source_id: i64,
        sort: PendingSort,
    ) -> Result<Vec<(crate::models::Series, crate::models::Episode)>> {
        let dir = match sort {
            PendingSort::RemainingAsc => "ASC",
            PendingSort::RemainingDesc => "DESC",
        };
        let sql = format!(
            "SELECT s.id, s.slug, s.title, s.url, s.cover_url, s.is_airing, s.followed,
                    e.id, e.series_id, e.number, e.title, e.url, e.released_at, e.seen,
                    s.next_episode_at, s.site_episode_count,
                    COUNT(*) OVER (PARTITION BY s.id) AS remaining
             FROM episodes e JOIN series s ON s.id = e.series_id
             WHERE e.seen=0 AND s.followed=1 AND s.source_id=?1
             ORDER BY remaining {dir}, s.title, e.added_at DESC"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt
            .query_map([source_id], |r| {
                let series = crate::models::Series {
                    id: r.get(0)?,
                    slug: r.get(1)?,
                    title: r.get(2)?,
                    url: r.get(3)?,
                    cover_url: r.get(4)?,
                    is_airing: r.get::<_, i64>(5)? != 0,
                    followed: r.get::<_, i64>(6)? != 0,
                    next_episode_at: r.get("next_episode_at")?,
                    site_episode_count: r.get("site_episode_count")?,
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

    /// Insert or refresh one AniList catalog entry, keyed by AniList's own
    /// numeric id (reused as our primary key — no reason to mint a separate
    /// one). `sort_order` is the position in the popularity-sorted sync
    /// sequence, so local pagination preserves the same ordering AniList's
    /// own `POPULARITY_DESC` sort gave it.
    pub fn upsert_catalog_anime(
        &self,
        anime: &crate::anilist::CatalogAnime,
        sort_order: i64,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO anilist_catalog(id, title, title_romaji, title_english, cover_url, format, episodes, average_score, popularity, url, sort_order)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
             ON CONFLICT(id) DO UPDATE SET
                title=excluded.title, title_romaji=excluded.title_romaji, title_english=excluded.title_english,
                cover_url=excluded.cover_url, format=excluded.format,
                episodes=excluded.episodes, average_score=excluded.average_score,
                popularity=excluded.popularity, url=excluded.url, sort_order=excluded.sort_order",
            (
                anime.id, &anime.title, &anime.title_romaji, &anime.title_english, &anime.cover_url, &anime.format,
                anime.episodes, anime.average_score, anime.popularity, &anime.url, sort_order,
            ),
        )?;
        self.conn.execute("DELETE FROM anilist_catalog_genres WHERE anilist_id=?1", [anime.id])?;
        for genre in &anime.genres {
            self.conn.execute(
                "INSERT OR IGNORE INTO anilist_catalog_genres(anilist_id, genre) VALUES(?1, ?2)",
                (anime.id, genre),
            )?;
        }
        Ok(())
    }

    /// How many entries are synced locally — lets the frontend show sync
    /// progress and decide whether a first-time sync is needed. Always the
    /// *unfiltered* total (header text); see `catalog_count_filtered` for a
    /// search/filter-scoped count.
    pub fn catalog_count(&self) -> Result<i64> {
        self.catalog_count_filtered(&CatalogFilter::default())
    }

    /// Builds the `WHERE` clause (always non-empty — starts from `1=1`) and
    /// positional params shared by `list_catalog_filtered` and
    /// `catalog_count_filtered`, so the two never drift out of sync with
    /// each other. Anonymous `?` placeholders are bound in the order pushed.
    fn build_catalog_where(filter: &CatalogFilter) -> (String, Vec<Value>) {
        let mut conditions: Vec<String> = vec!["1=1".to_string()];
        let mut params: Vec<Value> = Vec::new();

        let search = filter.search.as_deref().map(str::trim).filter(|s| !s.is_empty());
        if let Some(search) = search {
            conditions.push("title LIKE '%' || ? || '%' COLLATE NOCASE".to_string());
            params.push(Value::Text(search.to_string()));
        }

        let format = filter.format.as_deref().map(str::trim).filter(|s| !s.is_empty());
        if let Some(format) = format {
            conditions.push("format = ?".to_string());
            params.push(Value::Text(format.to_string()));
        }

        if let Some(min_score) = filter.min_score {
            conditions.push("average_score >= ?".to_string());
            params.push(Value::Integer(min_score));
        }

        let bucket = filter.episodes.as_deref().map(str::trim).filter(|s| !s.is_empty());
        if let Some(bucket) = bucket {
            match bucket {
                "1" => conditions.push("episodes = 1".to_string()),
                "2-12" => conditions.push("episodes BETWEEN 2 AND 12".to_string()),
                "13-26" => conditions.push("episodes BETWEEN 13 AND 26".to_string()),
                "27+" => conditions.push("episodes >= 27".to_string()),
                "unknown" => conditions.push("episodes IS NULL".to_string()),
                // Unrecognized bucket value: ignore rather than error, the
                // frontend only ever sends the five known values.
                _ => {}
            }
        }

        let genres: Vec<&str> = filter
            .genres
            .iter()
            .map(|g| g.trim())
            .filter(|g| !g.is_empty())
            .collect();
        if !genres.is_empty() {
            let placeholders = genres.iter().map(|_| "?").collect::<Vec<_>>().join(", ");
            conditions.push(format!(
                "id IN (SELECT anilist_id FROM anilist_catalog_genres \
                 WHERE genre IN ({placeholders}) GROUP BY anilist_id \
                 HAVING COUNT(DISTINCT genre) = ?)"
            ));
            for genre in &genres {
                params.push(Value::Text((*genre).to_string()));
            }
            params.push(Value::Integer(genres.len() as i64));
        }

        (conditions.join(" AND "), params)
    }

    /// Filtered + paginated variant of `list_catalog` — same popularity
    /// ordering, narrowed by `filter`. See `CatalogFilter` for field
    /// semantics and `build_catalog_where` for the query construction
    /// shared with `catalog_count_filtered`.
    pub fn list_catalog_filtered(
        &self,
        page: i64,
        per_page: i64,
        filter: &CatalogFilter,
    ) -> Result<Vec<crate::anilist::CatalogAnime>> {
        let offset = (page.max(1) - 1) * per_page;
        let (where_sql, mut params) = Self::build_catalog_where(filter);
        let sql = format!(
            "SELECT id, title, title_romaji, title_english, cover_url, format, episodes, average_score, popularity, url
             FROM anilist_catalog WHERE {where_sql}
             ORDER BY popularity DESC NULLS LAST, id LIMIT ? OFFSET ?"
        );
        params.push(Value::Integer(per_page));
        params.push(Value::Integer(offset));

        let mut stmt = self.conn.prepare(&sql)?;
        let mut rows = stmt
            .query_map(rusqlite::params_from_iter(params.iter()), Self::row_to_catalog_anime)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for anime in &mut rows {
            anime.genres = self.list_catalog_genres(anime.id)?;
        }
        Ok(rows)
    }

    /// Row count for a given filter — same `WHERE` as `list_catalog_filtered`,
    /// so paging through all pages of a filter always sums to this.
    pub fn catalog_count_filtered(&self, filter: &CatalogFilter) -> Result<i64> {
        let (where_sql, params) = Self::build_catalog_where(filter);
        let sql = format!("SELECT COUNT(*) FROM anilist_catalog WHERE {where_sql}");
        let mut stmt = self.conn.prepare(&sql)?;
        let count: i64 = stmt.query_row(rusqlite::params_from_iter(params.iter()), |r| r.get(0))?;
        Ok(count)
    }

    /// Distinct genre vocabulary from the synced catalog, alphabetical —
    /// drives the Catálogo filter bar's genre chips without hardcoding
    /// AniList's genre list.
    pub fn distinct_catalog_genres(&self) -> Result<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT DISTINCT genre FROM anilist_catalog_genres ORDER BY genre COLLATE NOCASE")?;
        let genres = stmt
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(genres)
    }

    /// Distinct, non-null format vocabulary from the synced catalog,
    /// alphabetical — drives the Catálogo filter bar's format select.
    pub fn distinct_catalog_formats(&self) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT format FROM anilist_catalog WHERE format IS NOT NULL ORDER BY format COLLATE NOCASE",
        )?;
        let formats = stmt
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(formats)
    }

    fn row_to_catalog_anime(r: &rusqlite::Row) -> rusqlite::Result<crate::anilist::CatalogAnime> {
        let id: i64 = r.get("id")?;
        Ok(crate::anilist::CatalogAnime {
            id,
            title: r.get("title")?,
            title_romaji: r.get("title_romaji")?,
            title_english: r.get("title_english")?,
            cover_url: r.get("cover_url")?,
            format: r.get("format")?,
            episodes: r.get("episodes")?,
            average_score: r.get("average_score")?,
            popularity: r.get("popularity")?,
            url: r.get("url")?,
            genres: Vec::new(), // filled in by callers that need it — see list_catalog
        })
    }

    /// One page of the locally-synced catalog, most-popular first. Popularity
    /// (not `sort_order`) drives ordering because the sync no longer crawls
    /// in popularity order (it's partitioned by id/date for completeness —
    /// see anilist.rs) — entries with no synced popularity (partial sync,
    /// or a title AniList reports no popularity for) sort last, tie-broken
    /// by id for stable pagination. Genres aren't joined in bulk here (kept
    /// to the simple per-row `list_catalog_genres` pattern already used for
    /// `series_genres` elsewhere) — fine at page-sized (30ish) result sets.
    pub fn list_catalog(&self, page: i64, per_page: i64) -> Result<Vec<crate::anilist::CatalogAnime>> {
        self.list_catalog_filtered(page, per_page, &CatalogFilter::default())
    }

    pub fn list_catalog_genres(&self, anilist_id: i64) -> Result<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT genre FROM anilist_catalog_genres WHERE anilist_id=?1 ORDER BY genre")?;
        let genres = stmt
            .query_map([anilist_id], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(genres)
    }

    /// A random synced entry carrying `genre`, subject to a quality floor —
    /// powers Descubrir's catalog-only swipe deck. The command layer
    /// (`discover_catalog_card`) owns the taste-weighted *genre* pick (via
    /// `get_genre_affinity` + `weighted_pick_index`, mirroring the site
    /// path); this just does the uniform-random pick *within* that genre,
    /// same division of labor `random_catalog_anime`'s callers already used.
    ///
    /// Quality floor: `format` restricted to the "real anime" formats
    /// (drops `MUSIC` and manga-side formats like `TV_SHORT` isn't excluded
    /// by name but is excluded implicitly by not being in this list), and
    /// `popularity >= 500` to cut the long tail of essentially-unknown
    /// entries. Already-decided titles (a `series` row with this
    /// `anilist_id`) are excluded so the deck never re-offers a decided
    /// card — see the `anilist_id` column comment in `init_schema`.
    ///
    /// `banned_formats` (from `get_banned_formats`) is subtracted from the
    /// default whitelist `['TV','MOVIE','OVA','ONA','SPECIAL']` to build the
    /// actual `IN (...)` clause dynamically — an unrecognized entry is
    /// harmlessly ignored (nothing in the whitelist matches it to remove).
    /// If every whitelisted format ends up banned, returns `Ok(None)`
    /// without querying — an empty SQL `IN ()` is invalid, and there's
    /// nothing left to offer anyway.
    pub fn random_catalog_anime_in_genre(
        &self,
        genre: &str,
        banned_formats: &[String],
    ) -> Result<Option<crate::anilist::CatalogAnime>> {
        const MIN_POPULARITY: i64 = 500;
        const DEFAULT_FORMATS: &[&str] = &["TV", "MOVIE", "OVA", "ONA", "SPECIAL"];
        let allowed: Vec<&str> = DEFAULT_FORMATS
            .iter()
            .copied()
            .filter(|f| !banned_formats.iter().any(|b| b.eq_ignore_ascii_case(f)))
            .collect();
        if allowed.is_empty() {
            return Ok(None);
        }
        let placeholders = allowed.iter().map(|_| "?").collect::<Vec<_>>().join(", ");
        let sql = format!(
            "SELECT c.id, c.title, c.title_romaji, c.title_english, c.cover_url, c.format, c.episodes, c.average_score, c.popularity, c.url
             FROM anilist_catalog c
             JOIN anilist_catalog_genres g ON g.anilist_id = c.id
             WHERE g.genre = ?
               AND c.format IN ({placeholders})
               AND c.popularity >= ?
               AND c.id NOT IN (SELECT anilist_id FROM series WHERE anilist_id IS NOT NULL)
             ORDER BY RANDOM() LIMIT 1"
        );
        let mut params: Vec<Value> = vec![Value::Text(genre.to_string())];
        for f in &allowed {
            params.push(Value::Text((*f).to_string()));
        }
        params.push(Value::Integer(MIN_POPULARITY));
        let mut stmt = self.conn.prepare(&sql)?;
        let mut rows = stmt.query_map(rusqlite::params_from_iter(params.iter()), Self::row_to_catalog_anime)?;
        let Some(row) = rows.next() else { return Ok(None) };
        let mut anime = row?;
        anime.genres = self.list_catalog_genres(anime.id)?;
        Ok(Some(anime))
    }

    /// The subset of a `series` row's fields the swipe-history strip needs
    /// to render one entry (`commands::list_swipe_history`): title, poster,
    /// and the two decision signals a catalog-swipe row can carry
    /// (`backlog_status`, `watched_externally` — catalog rows are never
    /// `followed` by `decide_catalog_card`, so that flag isn't needed here).
    /// `None` when the row no longer exists (already deleted by an earlier
    /// undo/return-to-deck) — the caller skips it rather than erroring, so
    /// the history strip self-heals.
    pub fn get_series_for_history(&self, series_id: i64) -> Result<Option<SwipeHistoryRow>> {
        Ok(self
            .conn
            .query_row(
                "SELECT title, cover_url, backlog_status, watched_externally FROM series WHERE id=?1",
                [series_id],
                |r| {
                    Ok(SwipeHistoryRow {
                        title: r.get(0)?,
                        poster_url: r.get(1)?,
                        backlog_status: r.get(2)?,
                        watched_externally: r.get::<_, i64>(3)? != 0,
                    })
                },
            )
            .ok())
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

    /// Minimal airing-row builder for the sort/skip-metadata tests below.
    fn mk_airing(slug: &str, title: &str, next_episode_at: Option<i64>) -> crate::models::Series {
        crate::models::Series {
            id: 0,
            slug: slug.into(),
            title: title.into(),
            url: format!("https://site/tv/{slug}/"),
            cover_url: None,
            is_airing: true,
            followed: false,
            next_episode_at,
            site_episode_count: None,
        }
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

    #[test]
    fn episode_count_counts_only_this_series() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
        let sid_a = db.upsert_series(src, &mk_airing("a", "A", None)).unwrap();
        let sid_b = db.upsert_series(src, &mk_airing("b", "B", None)).unwrap();
        for i in 1..=3 {
            db.insert_episode(&crate::models::Episode {
                id: 0, series_id: sid_a, number: i.to_string(), title: None,
                url: format!("https://site/a-{i}"), released_at: None, seen: false,
            }).unwrap();
        }
        assert_eq!(db.episode_count(sid_a).unwrap(), 3);
        assert_eq!(db.episode_count(sid_b).unwrap(), 0);
    }

    /// `max_episode_number` must reflect the highest real episode number, not
    /// the row count — recap/"0"/version-reupload rows must never inflate it
    /// (Bug B in the 2026-07-12 airing-refresh-missing-episodes fix: a row
    /// COUNT was being compared against the site's next-episode-number badge,
    /// so a recap row cancelled the detection margin and a real new episode
    /// got silently skipped).
    #[test]
    fn max_episode_number_ignores_recap_and_version_rows() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();

        // Wistoria-shaped: "0|Recap", "1".."12" -> 13 rows, highest real = 12.
        let sid_a = db.upsert_series(src, &mk_airing("a", "A", None)).unwrap();
        db.insert_episode(&crate::models::Episode {
            id: 0, series_id: sid_a, number: "0 | Recap".to_string(), title: None,
            url: "https://site/a-recap".to_string(), released_at: None, seen: false,
        }).unwrap();
        for i in 1..=12 {
            db.insert_episode(&crate::models::Episode {
                id: 0, series_id: sid_a, number: i.to_string(), title: None,
                url: format!("https://site/a-{i}"), released_at: None, seen: false,
            }).unwrap();
        }
        assert_eq!(db.episode_count(sid_a).unwrap(), 13, "sanity: row count is 13");
        assert_eq!(db.max_episode_number(sid_a).unwrap(), 12);

        // Tomb Raider King-shaped: "1 | v2", "2 | v1" -> highest real = 2.
        let sid_b = db.upsert_series(src, &mk_airing("b", "B", None)).unwrap();
        db.insert_episode(&crate::models::Episode {
            id: 0, series_id: sid_b, number: "1 | v2".to_string(), title: None,
            url: "https://site/b-1".to_string(), released_at: None, seen: false,
        }).unwrap();
        db.insert_episode(&crate::models::Episode {
            id: 0, series_id: sid_b, number: "2 | v1".to_string(), title: None,
            url: "https://site/b-2".to_string(), released_at: None, seen: false,
        }).unwrap();
        assert_eq!(db.max_episode_number(sid_b).unwrap(), 2);

        // No episodes at all -> 0.
        let sid_c = db.upsert_series(src, &mk_airing("c", "C", None)).unwrap();
        assert_eq!(db.max_episode_number(sid_c).unwrap(), 0);
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

    /// Airing-sort acceptance test (see the airing-sort-order design doc):
    /// newest-first by next_episode_at, NULLs last, title as the stable
    /// tie-break for equal timestamps.
    #[test]
    fn list_airing_orders_newest_first_nulls_last_title_tiebreak() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
        db.upsert_series(src, &mk_airing("older", "Older", Some(1_000_000))).unwrap();
        db.upsert_series(src, &mk_airing("newer", "Newer", Some(2_000_000))).unwrap();
        db.upsert_series(src, &mk_airing("nodate", "NoDate", None)).unwrap();
        db.upsert_series(src, &mk_airing("tie-b", "Tie B", Some(1_000_000))).unwrap();

        let titles: Vec<String> = db.list_airing(src).unwrap().into_iter().map(|s| s.title).collect();
        assert_eq!(titles, vec!["Newer", "Older", "Tie B", "NoDate"]);
    }

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
    fn seen_cascade_handles_non_integer_episode_numbers() {
        // Regression: episode numbers like "1x05" (season-prefixed) used to
        // fail Rust's strict i64 parse (-> 0) while SQLite's CAST still read
        // the leading digit, so un-marking one used to wipe every episode's
        // seen flag in the series instead of just the later ones.
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "https://wwv.animeytx.net", "animeytx").unwrap();
        let s = crate::models::Series {
            id: 0, slug: "x".into(), title: "X".into(),
            url: "u".into(), cover_url: None, is_airing: true, followed: false, next_episode_at: None, site_episode_count: None,
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
    fn set_seen_cascade_stamps_and_clears_seen_at() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "https://wwv.animeytx.net", "animeytx").unwrap();
        let s = crate::models::Series {
            id: 0, slug: "x".into(), title: "X".into(),
            url: "u".into(), cover_url: None, is_airing: true, followed: false, next_episode_at: None, site_episode_count: None,
        };
        let sid = db.upsert_series(src, &s).unwrap();
        let mk = |n: &str, url: &str| crate::models::Episode {
            id: 0, series_id: sid, number: n.into(), title: None,
            url: url.into(), released_at: None, seen: false,
        };
        let e1 = db.insert_episode(&mk("1", "https://site/e1")).unwrap();
        let e2 = db.insert_episode(&mk("2", "https://site/e2")).unwrap();

        let seen_at = |id: i64| -> Option<String> {
            db.conn
                .query_row("SELECT seen_at FROM episodes WHERE id=?1", [id], |r| r.get(0))
                .unwrap()
        };
        assert!(seen_at(e1).is_none());
        assert!(seen_at(e2).is_none());

        // marking "2" seen cascades to "1" — both rows get seen_at, not just
        // the one explicitly clicked.
        db.set_seen_cascade(sid, "2", true).unwrap();
        assert!(seen_at(e1).is_some());
        assert!(seen_at(e2).is_some());

        // un-marking "2" clears seen_at on the cascade target too
        db.set_seen_cascade(sid, "2", false).unwrap();
        assert!(seen_at(e1).is_some(), "e1 stays seen, keeps its seen_at");
        assert!(seen_at(e2).is_none(), "e2 un-marked, seen_at cleared");
    }

    #[test]
    fn set_seen_stamps_and_clears_seen_at() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "https://wwv.animeytx.net", "animeytx").unwrap();
        let s = crate::models::Series {
            id: 0, slug: "x".into(), title: "X".into(),
            url: "u".into(), cover_url: None, is_airing: true, followed: false, next_episode_at: None, site_episode_count: None,
        };
        let sid = db.upsert_series(src, &s).unwrap();
        let e1 = db
            .insert_episode(&crate::models::Episode {
                id: 0, series_id: sid, number: "1".into(), title: None,
                url: "https://site/e1".into(), released_at: None, seen: false,
            })
            .unwrap();

        db.set_seen(e1, true).unwrap();
        let seen_at: Option<String> = db
            .conn
            .query_row("SELECT seen_at FROM episodes WHERE id=?1", [e1], |r| r.get(0))
            .unwrap();
        assert!(seen_at.is_some());

        db.set_seen(e1, false).unwrap();
        let seen_at: Option<String> = db
            .conn
            .query_row("SELECT seen_at FROM episodes WHERE id=?1", [e1], |r| r.get(0))
            .unwrap();
        assert!(seen_at.is_none());
    }

    #[test]
    fn library_next_episode_agrees_with_series_detail_ordering() {
        // "9", "10", "10.5" — the design doc's canonical case for why next-
        // episode ordering must reuse list_series_episodes' ORDER BY
        // (CAST(number AS INTEGER) ASC, id ASC) rather than a numeric parse:
        // a naive string sort would put "10" before "9".
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "https://wwv.animeytx.net", "animeytx").unwrap();
        let s = crate::models::Series {
            id: 0, slug: "x".into(), title: "X".into(),
            url: "u".into(), cover_url: None, is_airing: true, followed: true, next_episode_at: None, site_episode_count: None,
        };
        let sid = db.upsert_series(src, &s).unwrap();
        // upsert_series never sets `followed` (scan path must not un-follow);
        // list_library filters on followed=1, so follow it explicitly.
        db.set_followed(sid, true).unwrap();
        let mk = |n: &str, url: &str| crate::models::Episode {
            id: 0, series_id: sid, number: n.into(), title: None,
            url: url.into(), released_at: None, seen: false,
        };
        // Insert out of numeric order to make sure ordering isn't just
        // reflecting insertion order.
        db.insert_episode(&mk("10", "https://site/e10")).unwrap();
        db.insert_episode(&mk("9", "https://site/e9")).unwrap();
        db.insert_episode(&mk("10.5", "https://site/e10.5")).unwrap();

        let detail_order = db.list_series_episodes(sid).unwrap();
        let first_unseen_in_detail_order = detail_order.iter().find(|e| !e.seen).unwrap();

        let items = db.list_library(src).unwrap();
        let item = items.iter().find(|it| it.series.id == sid).unwrap();
        let next = item.next_episode.as_ref().expect("has an unseen episode");
        assert_eq!(next.number, first_unseen_in_detail_order.number);
        assert_eq!(next.number, "9", "lowest by SeriesDetail's own ordering, not string sort");

        // Mark "9" seen; next should become "10" (not "10.5" — 10 sorts
        // before 10.5 under CAST(number AS INTEGER) since both cast to 10,
        // then id ASC breaks the tie in insertion order: "10" was inserted
        // before "10.5").
        db.set_seen_cascade(sid, "9", true).unwrap();
        let items = db.list_library(src).unwrap();
        let item = items.iter().find(|it| it.series.id == sid).unwrap();
        assert_eq!(item.next_episode.as_ref().unwrap().number, "10");
    }

    #[test]
    fn library_next_episode_none_when_fully_seen() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "https://wwv.animeytx.net", "animeytx").unwrap();
        let s = crate::models::Series {
            id: 0, slug: "x".into(), title: "X".into(),
            url: "u".into(), cover_url: None, is_airing: true, followed: true, next_episode_at: None, site_episode_count: None,
        };
        let sid = db.upsert_series(src, &s).unwrap();
        db.set_followed(sid, true).unwrap();
        db.insert_episode(&crate::models::Episode {
            id: 0, series_id: sid, number: "1".into(), title: None,
            url: "https://site/e1".into(), released_at: None, seen: false,
        })
        .unwrap();
        db.set_seen_cascade(sid, "1", true).unwrap();

        let items = db.list_library(src).unwrap();
        let item = items.iter().find(|it| it.series.id == sid).unwrap();
        assert!(item.next_episode.is_none());
        assert_eq!(item.total_episodes, 1);
        assert_eq!(item.seen_episodes, 1);
    }

    #[test]
    fn library_next_episode_none_and_no_panic_with_zero_episodes() {
        // A followed-but-never-scraped (or scrape-failed) series: total=0.
        // Must not divide by zero anywhere and must yield next_episode=None.
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "https://wwv.animeytx.net", "animeytx").unwrap();
        let s = crate::models::Series {
            id: 0, slug: "x".into(), title: "X".into(),
            url: "u".into(), cover_url: None, is_airing: true, followed: true, next_episode_at: None, site_episode_count: None,
        };
        let sid = db.upsert_series(src, &s).unwrap();
        db.set_followed(sid, true).unwrap();

        let items = db.list_library(src).unwrap();
        assert_eq!(items.len(), 1);
        assert!(items[0].next_episode.is_none());
        assert_eq!(items[0].total_episodes, 0);
        assert_eq!(items[0].seen_episodes, 0);
    }

    #[test]
    fn insert_episode_dedups_and_marks_seen() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "https://wwv.animeytx.net", "animeytx").unwrap();
        let s = crate::models::Series {
            id: 0, slug: "x".into(), title: "X".into(),
            url: "u".into(), cover_url: None, is_airing: true, followed: false, next_episode_at: None, site_episode_count: None,
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

        assert_eq!(db.pending_count(src).unwrap(), 1);
        db.set_seen(eid, true).unwrap();
        assert_eq!(db.pending_count(src).unwrap(), 0);
    }

    #[test]
    fn pending_count_and_list_pending_are_scoped_per_source() {
        let db = Db::open(":memory:").unwrap();
        let src_a = db.upsert_source("AnimeYT", "https://a.example", "animeytx").unwrap();
        let src_b = db.upsert_source("TioAnime", "https://b.example", "tioanime").unwrap();

        let mk = |slug: &str| crate::models::Series {
            id: 0, slug: slug.into(), title: slug.into(), url: format!("u-{slug}"),
            cover_url: None, is_airing: true, followed: false, next_episode_at: None, site_episode_count: None,
        };
        let sid_a = db.upsert_series(src_a, &mk("a")).unwrap();
        db.set_followed(sid_a, true).unwrap();
        let sid_b = db.upsert_series(src_b, &mk("b")).unwrap();
        db.set_followed(sid_b, true).unwrap();

        db.insert_episode(&crate::models::Episode {
            id: 0, series_id: sid_a, number: "1".into(), title: None,
            url: "https://a.example/ep1".into(), released_at: None, seen: false,
        })
        .unwrap();
        db.insert_episode(&crate::models::Episode {
            id: 0, series_id: sid_b, number: "1".into(), title: None,
            url: "https://b.example/ep1".into(), released_at: None, seen: false,
        })
        .unwrap();
        db.insert_episode(&crate::models::Episode {
            id: 0, series_id: sid_b, number: "2".into(), title: None,
            url: "https://b.example/ep2".into(), released_at: None, seen: false,
        })
        .unwrap();

        assert_eq!(db.pending_count(src_a).unwrap(), 1);
        assert_eq!(db.pending_count(src_b).unwrap(), 2);
        assert_eq!(db.list_pending(src_a, PendingSort::RemainingAsc).unwrap().len(), 1);
        assert_eq!(db.list_pending(src_b, PendingSort::RemainingAsc).unwrap().len(), 2);
        assert!(db
            .list_pending(src_a, PendingSort::RemainingAsc)
            .unwrap()
            .iter()
            .all(|(s, _)| s.id == sid_a));
    }

    #[test]
    fn list_pending_orders_groups_by_remaining_count() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "https://a.example", "animeytx").unwrap();
        let mk = |slug: &str| crate::models::Series {
            id: 0, slug: slug.into(), title: slug.into(), url: format!("u-{slug}"),
            cover_url: None, is_airing: true, followed: false, next_episode_at: None, site_episode_count: None,
        };
        // "few" has 1 pending episode, "many" has 3.
        let few = db.upsert_series(src, &mk("few")).unwrap();
        db.set_followed(few, true).unwrap();
        let many = db.upsert_series(src, &mk("many")).unwrap();
        db.set_followed(many, true).unwrap();
        db.insert_episode(&crate::models::Episode {
            id: 0, series_id: few, number: "1".into(), title: None,
            url: "few/1".into(), released_at: None, seen: false,
        }).unwrap();
        for n in 1..=3 {
            db.insert_episode(&crate::models::Episode {
                id: 0, series_id: many, number: n.to_string(), title: None,
                url: format!("many/{n}"), released_at: None, seen: false,
            }).unwrap();
        }

        // Ascending: the 1-episode series' rows come before the 3-episode one.
        let asc = db.list_pending(src, PendingSort::RemainingAsc).unwrap();
        assert_eq!(asc.first().unwrap().0.id, few);
        assert_eq!(asc.last().unwrap().0.id, many);
        // Descending: reversed.
        let desc = db.list_pending(src, PendingSort::RemainingDesc).unwrap();
        assert_eq!(desc.first().unwrap().0.id, many);
        assert_eq!(desc.last().unwrap().0.id, few);
        // Each series' episodes stay contiguous (no interleaving).
        let ids: Vec<i64> = asc.iter().map(|(s, _)| s.id).collect();
        assert_eq!(ids, vec![few, many, many, many]);
    }

    #[test]
    fn existing_episode_urls_returns_known_urls() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
        let s = crate::models::Series {
            id: 0, slug: "x".into(), title: "X".into(),
            url: "u".into(), cover_url: None, is_airing: true, followed: true, next_episode_at: None, site_episode_count: None,
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
    fn get_genre_affinity_weighs_followed_want_discarded_correctly() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();

        let followed = crate::models::Series {
            id: 0, slug: "f".into(), title: "F".into(),
            url: "u1".into(), cover_url: None, is_airing: false, followed: true, next_episode_at: None, site_episode_count: None,
        };
        let sid_f = db.upsert_series(src, &followed).unwrap();
        db.set_followed(sid_f, true).unwrap();
        db.insert_series_genres(sid_f, &["Seinen".to_string(), "Shared".to_string()]).unwrap();

        let want = crate::models::Series {
            id: 0, slug: "w".into(), title: "W".into(),
            url: "u2".into(), cover_url: None, is_airing: false, followed: false, next_episode_at: None, site_episode_count: None,
        };
        let sid_w = db.upsert_series(src, &want).unwrap();
        db.set_backlog_status(sid_w, Some("want")).unwrap();
        db.insert_series_genres(sid_w, &["Romance".to_string(), "Shared".to_string()]).unwrap();

        let discarded = crate::models::Series {
            id: 0, slug: "d".into(), title: "D".into(),
            url: "u3".into(), cover_url: None, is_airing: false, followed: false, next_episode_at: None, site_episode_count: None,
        };
        let sid_d = db.upsert_series(src, &discarded).unwrap();
        db.set_backlog_status(sid_d, Some("discarded")).unwrap();
        db.insert_series_genres(sid_d, &["Horror".to_string()]).unwrap();

        let scores = db.get_genre_affinity(src).unwrap();
        assert_eq!(scores.get("Seinen"), Some(&2.0));
        assert_eq!(scores.get("Romance"), Some(&1.0));
        assert_eq!(scores.get("Horror"), Some(&-1.5));
        assert_eq!(scores.get("Shared"), Some(&3.0)); // 2.0 (followed) + 1.0 (want)
        assert_eq!(scores.get("Unseen"), None);
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
    fn get_catalog_titles_reads_all_three_title_fields() {
        let db = Db::open(":memory:").unwrap();
        let anime = crate::anilist::CatalogAnime {
            id: 42,
            title: "Attack on Titan".into(),
            title_romaji: Some("Shingeki no Kyojin".into()),
            title_english: Some("Attack on Titan".into()),
            cover_url: None,
            format: Some("TV".into()),
            genres: vec!["Action".into()],
            episodes: Some(25),
            average_score: Some(90),
            popularity: Some(1000),
            url: "https://anilist.co/anime/42".into(),
        };
        db.upsert_catalog_anime(&anime, 0).unwrap();

        let (title, romaji, english) = db.get_catalog_titles(42).unwrap().unwrap();
        assert_eq!(title, "Attack on Titan");
        assert_eq!(romaji.as_deref(), Some("Shingeki no Kyojin"));
        assert_eq!(english.as_deref(), Some("Attack on Titan"));
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

    /// Acceptance criterion: "watched_externally + link marks all episodes
    /// seen" — after episodes are scraped in for a title the user decided
    /// `Seen` on the catalog deck, every episode must end up seen, via the
    /// same gap-free `set_seen_cascade` the rest of the app's watch-tracking
    /// uses (not a separate blanket UPDATE).
    #[test]
    fn mark_all_episodes_seen_marks_every_episode_via_cascade() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
        let s = crate::models::Series {
            id: 0, slug: "x".into(), title: "X".into(),
            url: "u".into(), cover_url: None, is_airing: false, followed: false, next_episode_at: None, site_episode_count: None,
        };
        let sid = db.upsert_series(src, &s).unwrap();
        for n in ["1", "2", "3"] {
            db.insert_episode(&crate::models::Episode {
                id: 0, series_id: sid, number: n.into(), title: None,
                url: format!("e{n}"), released_at: None, seen: false,
            }).unwrap();
        }

        db.mark_all_episodes_seen(sid).unwrap();

        let eps = db.list_series_episodes(sid).unwrap();
        assert_eq!(eps.len(), 3);
        assert!(eps.iter().all(|e| e.seen), "every episode must be marked seen");
    }

    #[test]
    fn mark_all_episodes_seen_is_a_no_op_on_a_series_with_no_episodes() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
        let s = crate::models::Series {
            id: 0, slug: "x".into(), title: "X".into(),
            url: "u".into(), cover_url: None, is_airing: false, followed: false, next_episode_at: None, site_episode_count: None,
        };
        let sid = db.upsert_series(src, &s).unwrap();
        db.mark_all_episodes_seen(sid).unwrap(); // must not error on zero episodes
        assert!(db.list_series_episodes(sid).unwrap().is_empty());
    }

    #[test]
    fn known_series_urls_reflects_any_decided_row() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
        let s = crate::models::Series {
            id: 0, slug: "x".into(), title: "X".into(),
            url: "https://site/tv/x/".into(), cover_url: None, is_airing: false, followed: false, next_episode_at: None, site_episode_count: None,
        };
        let sid = db.upsert_series(src, &s).unwrap();
        db.set_backlog_status(sid, Some("discarded")).unwrap();

        let known = db.known_series_urls(src).unwrap();
        assert!(known.contains("https://site/tv/x/"));
        assert_eq!(known.len(), 1);
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
    fn get_stats_graph_data_returns_genres_kind_and_cover_for_followed_series() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();

        let full = crate::models::Series {
            id: 0, slug: "full".into(), title: "Full".into(),
            url: "u1".into(), cover_url: Some("data:image/png;base64,x".into()),
            is_airing: false, followed: true, next_episode_at: None, site_episode_count: None,
        };
        let sid_full = db.upsert_series(src, &full).unwrap();
        db.set_followed(sid_full, true).unwrap();
        db.insert_series_genres(sid_full, &["Seinen".to_string(), "Drama".to_string()]).unwrap();
        db.set_kind(sid_full, "TV").unwrap();

        let bare = crate::models::Series {
            id: 0, slug: "bare".into(), title: "Bare".into(),
            url: "u2".into(), cover_url: None, is_airing: false, followed: true, next_episode_at: None, site_episode_count: None,
        };
        let sid_bare = db.upsert_series(src, &bare).unwrap();
        db.set_followed(sid_bare, true).unwrap();

        // not followed => excluded
        let other = crate::models::Series {
            id: 0, slug: "other".into(), title: "Other".into(),
            url: "u3".into(), cover_url: None, is_airing: false, followed: false, next_episode_at: None, site_episode_count: None,
        };
        db.upsert_series(src, &other).unwrap();

        let rows = db.get_stats_graph_data(src).unwrap();
        assert_eq!(rows.len(), 2);

        let full_row = rows.iter().find(|r| r.id == sid_full).unwrap();
        assert_eq!(full_row.genres, vec!["Drama".to_string(), "Seinen".to_string()]);
        assert_eq!(full_row.kind.as_deref(), Some("TV"));
        assert_eq!(full_row.cover_url.as_deref(), Some("data:image/png;base64,x"));

        let bare_row = rows.iter().find(|r| r.id == sid_bare).unwrap();
        assert!(bare_row.genres.is_empty());
        assert_eq!(bare_row.kind, None);
        assert_eq!(bare_row.cover_url, None);
    }

    fn catalog_anime(id: i64, title: &str, genres: &[&str]) -> crate::anilist::CatalogAnime {
        catalog_anime_with_popularity(id, title, genres, None)
    }

    fn catalog_anime_with_popularity(
        id: i64,
        title: &str,
        genres: &[&str],
        popularity: Option<i64>,
    ) -> crate::anilist::CatalogAnime {
        crate::anilist::CatalogAnime {
            id,
            title: title.into(),
            title_romaji: None,
            title_english: None,
            cover_url: Some(format!("https://cdn/{id}.jpg")),
            format: Some("TV".into()),
            genres: genres.iter().map(|g| g.to_string()).collect(),
            episodes: Some(12),
            average_score: Some(80),
            popularity,
            url: format!("https://anilist.co/anime/{id}"),
        }
    }

    #[test]
    fn catalog_upsert_and_list_orders_by_popularity_desc() {
        let db = Db::open(":memory:").unwrap();
        // Inserted out of popularity order and out of sort_order too, to
        // prove ordering follows `popularity`, not insertion or sort_order.
        db.upsert_catalog_anime(&catalog_anime_with_popularity(3, "LowPop", &["Drama"], Some(10)), 0).unwrap();
        db.upsert_catalog_anime(&catalog_anime_with_popularity(1, "HighPop", &["Action", "Fantasy"], Some(9000)), 1).unwrap();
        db.upsert_catalog_anime(&catalog_anime_with_popularity(2, "MidPop", &[], Some(500)), 2).unwrap();

        assert_eq!(db.catalog_count().unwrap(), 3);

        let page = db.list_catalog(1, 10).unwrap();
        assert_eq!(
            page.iter().map(|a| a.title.as_str()).collect::<Vec<_>>(),
            vec!["HighPop", "MidPop", "LowPop"]
        );
        assert_eq!(page[0].genres, vec!["Action".to_string(), "Fantasy".to_string()]);
    }

    #[test]
    fn catalog_list_puts_null_popularity_last_and_tie_breaks_by_id() {
        let db = Db::open(":memory:").unwrap();
        db.upsert_catalog_anime(&catalog_anime_with_popularity(5, "NoPopB", &[], None), 0).unwrap();
        db.upsert_catalog_anime(&catalog_anime_with_popularity(2, "NoPopA", &[], None), 1).unwrap();
        db.upsert_catalog_anime(&catalog_anime_with_popularity(1, "HasPop", &[], Some(50)), 2).unwrap();

        let page = db.list_catalog(1, 10).unwrap();
        assert_eq!(
            page.iter().map(|a| a.title.as_str()).collect::<Vec<_>>(),
            vec!["HasPop", "NoPopA", "NoPopB"]
        );
    }

    #[test]
    fn catalog_popularity_index_exists() {
        let db = Db::open(":memory:").unwrap();
        let name: String = db
            .conn
            .query_row(
                "SELECT name FROM sqlite_master WHERE type='index' AND name='idx_catalog_popularity'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(name, "idx_catalog_popularity");
    }

    #[test]
    fn catalog_genre_index_exists() {
        let db = Db::open(":memory:").unwrap();
        let name: String = db
            .conn
            .query_row(
                "SELECT name FROM sqlite_master WHERE type='index' AND name='idx_catalog_genre'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(name, "idx_catalog_genre");
    }

    #[test]
    fn catalog_upsert_is_idempotent_and_refreshes_genres() {
        let db = Db::open(":memory:").unwrap();
        db.upsert_catalog_anime(&catalog_anime(1, "First", &["Action"]), 0).unwrap();
        db.upsert_catalog_anime(&catalog_anime(1, "First (updated)", &["Comedy"]), 0).unwrap();

        assert_eq!(db.catalog_count().unwrap(), 1);
        let page = db.list_catalog(1, 10).unwrap();
        assert_eq!(page.len(), 1);
        assert_eq!(page[0].title, "First (updated)");
        assert_eq!(page[0].genres, vec!["Comedy".to_string()]);
    }

    #[test]
    fn catalog_pagination_offsets_correctly() {
        let db = Db::open(":memory:").unwrap();
        for i in 0..5 {
            db.upsert_catalog_anime(&catalog_anime(i, &format!("Anime {i}"), &[]), i).unwrap();
        }
        let page1 = db.list_catalog(1, 2).unwrap();
        let page2 = db.list_catalog(2, 2).unwrap();
        assert_eq!(page1.iter().map(|a| a.id).collect::<Vec<_>>(), vec![0, 1]);
        assert_eq!(page2.iter().map(|a| a.id).collect::<Vec<_>>(), vec![2, 3]);
    }

    #[test]
    fn random_catalog_anime_in_genre_none_when_empty_some_when_populated() {
        let db = Db::open(":memory:").unwrap();
        assert!(db.random_catalog_anime_in_genre("Drama", &[]).unwrap().is_none());
        db.upsert_catalog_anime(
            &catalog_anime_with_popularity(1, "Only", &["Drama"], Some(1000)),
            0,
        )
        .unwrap();
        let picked = db.random_catalog_anime_in_genre("Drama", &[]).unwrap().unwrap();
        assert_eq!(picked.id, 1);
        assert_eq!(picked.genres, vec!["Drama".to_string()]);
    }

    #[test]
    fn random_catalog_anime_in_genre_only_matches_requested_genre() {
        let db = Db::open(":memory:").unwrap();
        db.upsert_catalog_anime(&catalog_anime_with_popularity(1, "Dramatic", &["Drama"], Some(1000)), 0).unwrap();
        db.upsert_catalog_anime(&catalog_anime_with_popularity(2, "Actiony", &["Action"], Some(1000)), 1).unwrap();
        let picked = db.random_catalog_anime_in_genre("Action", &[]).unwrap().unwrap();
        assert_eq!(picked.title, "Actiony");
    }

    #[test]
    fn random_catalog_anime_in_genre_excludes_low_popularity_and_wrong_format() {
        let db = Db::open(":memory:").unwrap();
        // Below the 500 popularity floor -> excluded.
        db.upsert_catalog_anime(&catalog_anime_with_popularity(1, "TooObscure", &["Drama"], Some(50)), 0).unwrap();
        // MUSIC format -> excluded regardless of popularity.
        let mut music = catalog_anime_with_popularity(2, "MusicVideo", &["Drama"], Some(9000));
        music.format = Some("MUSIC".into());
        db.upsert_catalog_anime(&music, 1).unwrap();
        // Qualifies on both counts.
        db.upsert_catalog_anime(&catalog_anime_with_popularity(3, "Qualifies", &["Drama"], Some(1000)), 2).unwrap();

        let picked = db.random_catalog_anime_in_genre("Drama", &[]).unwrap().unwrap();
        assert_eq!(picked.title, "Qualifies");
    }

    #[test]
    fn random_catalog_anime_in_genre_excludes_already_decided_titles() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
        db.upsert_catalog_anime(&catalog_anime_with_popularity(1, "Decided", &["Drama"], Some(1000)), 0).unwrap();
        db.upsert_catalog_anime(&catalog_anime_with_popularity(2, "Undecided", &["Drama"], Some(1000)), 1).unwrap();

        let series = crate::models::Series {
            id: 0, slug: "anilist-1".into(), title: "Decided".into(),
            url: "https://anilist.co/anime/1".into(), cover_url: None,
            is_airing: false, followed: false, next_episode_at: None, site_episode_count: None,
        };
        let sid = db.upsert_series(src, &series).unwrap();
        db.set_anilist_id(sid, 1).unwrap();
        db.set_backlog_status(sid, Some("discarded")).unwrap();

        for _ in 0..10 {
            let picked = db.random_catalog_anime_in_genre("Drama", &[]).unwrap().unwrap();
            assert_eq!(picked.title, "Undecided");
        }
    }

    #[test]
    fn random_catalog_anime_in_genre_returns_none_when_all_decided() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
        db.upsert_catalog_anime(&catalog_anime_with_popularity(1, "OnlyOne", &["Drama"], Some(1000)), 0).unwrap();
        let series = crate::models::Series {
            id: 0, slug: "anilist-1".into(), title: "OnlyOne".into(),
            url: "https://anilist.co/anime/1".into(), cover_url: None,
            is_airing: false, followed: false, next_episode_at: None, site_episode_count: None,
        };
        let sid = db.upsert_series(src, &series).unwrap();
        db.set_anilist_id(sid, 1).unwrap();
        db.set_backlog_status(sid, Some("want")).unwrap();

        assert!(db.random_catalog_anime_in_genre("Drama", &[]).unwrap().is_none());
    }

    #[test]
    fn random_catalog_anime_in_genre_excludes_a_banned_format() {
        let db = Db::open(":memory:").unwrap();
        let mut movie = catalog_anime_with_popularity(1, "AMovie", &["Drama"], Some(1000));
        movie.format = Some("MOVIE".into());
        db.upsert_catalog_anime(&movie, 0).unwrap();
        let mut tv = catalog_anime_with_popularity(2, "ATVShow", &["Drama"], Some(1000));
        tv.format = Some("TV".into());
        db.upsert_catalog_anime(&tv, 1).unwrap();

        // With MOVIE banned, only the TV entry can ever be picked.
        for _ in 0..10 {
            let picked = db.random_catalog_anime_in_genre("Drama", &["MOVIE".to_string()]).unwrap().unwrap();
            assert_eq!(picked.title, "ATVShow");
        }
    }

    #[test]
    fn random_catalog_anime_in_genre_none_when_every_format_banned() {
        let db = Db::open(":memory:").unwrap();
        let mut tv = catalog_anime_with_popularity(1, "ATVShow", &["Drama"], Some(1000));
        tv.format = Some("TV".into());
        db.upsert_catalog_anime(&tv, 0).unwrap();

        let all_banned: Vec<String> =
            ["TV", "MOVIE", "OVA", "ONA", "SPECIAL"].iter().map(|s| s.to_string()).collect();
        // An invalid empty SQL IN () would error here if not short-circuited.
        assert!(db.random_catalog_anime_in_genre("Drama", &all_banned).unwrap().is_none());
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
    fn get_series_for_history_returns_none_for_a_deleted_row() {
        let db = Db::open(":memory:").unwrap();
        assert!(db.get_series_for_history(999).unwrap().is_none());
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

    #[test]
    fn get_genre_affinity_folds_spanish_and_english_genre_into_one_canonical_key() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();

        // A followed series tagged with the site's Spanish genre "Acción".
        let followed = crate::models::Series {
            id: 0, slug: "f".into(), title: "F".into(),
            url: "u1".into(), cover_url: None, is_airing: false, followed: true, next_episode_at: None, site_episode_count: None,
        };
        let sid_f = db.upsert_series(src, &followed).unwrap();
        db.set_followed(sid_f, true).unwrap();
        db.insert_series_genres(sid_f, &["Acción".to_string(), "Isekai".to_string()]).unwrap();

        // A 'want' series tagged with the catalog's English genre "Action".
        let want = crate::models::Series {
            id: 0, slug: "w".into(), title: "W".into(),
            url: "u2".into(), cover_url: None, is_airing: false, followed: false, next_episode_at: None, site_episode_count: None,
        };
        let sid_w = db.upsert_series(src, &want).unwrap();
        db.set_backlog_status(sid_w, Some("want")).unwrap();
        db.insert_series_genres(sid_w, &["Action".to_string()]).unwrap();

        let scores = db.get_genre_affinity(src).unwrap();
        // 2.0 (followed, "Acción" normalized to "Action") + 1.0 (want, "Action") = 3.0
        assert_eq!(scores.get("Action"), Some(&3.0));
        assert_eq!(scores.get("Acción"), None);
        // Site-only tag with no AniList equivalent keeps its raw name.
        assert_eq!(scores.get("Isekai"), Some(&2.0));
    }

    fn catalog_anime_full(
        id: i64,
        title: &str,
        genres: &[&str],
        format: &str,
        episodes: Option<i64>,
        average_score: Option<i64>,
    ) -> crate::anilist::CatalogAnime {
        crate::anilist::CatalogAnime {
            id,
            title: title.into(),
            title_romaji: None,
            title_english: None,
            cover_url: None,
            format: Some(format.into()),
            genres: genres.iter().map(|g| g.to_string()).collect(),
            episodes,
            average_score,
            popularity: Some(100 - id),
            url: format!("https://anilist.co/anime/{id}"),
        }
    }

    fn seed_filter_catalog(db: &Db) {
        db.upsert_catalog_anime(
            &catalog_anime_full(1, "Monster", &["Drama", "Mystery"], "TV", Some(74), Some(90)),
            0,
        )
        .unwrap();
        db.upsert_catalog_anime(
            &catalog_anime_full(2, "Monster Girl", &["Comedy"], "TV", Some(12), Some(65)),
            1,
        )
        .unwrap();
        db.upsert_catalog_anime(
            &catalog_anime_full(3, "Fullmetal Alchemist", &["Drama", "Action"], "TV", Some(64), Some(88)),
            2,
        )
        .unwrap();
        db.upsert_catalog_anime(
            &catalog_anime_full(4, "OnlyOne", &["Drama"], "MOVIE", Some(1), Some(70)),
            3,
        )
        .unwrap();
        db.upsert_catalog_anime(
            &catalog_anime_full(5, "ShortRun", &["Action"], "OVA", Some(6), Some(55)),
            4,
        )
        .unwrap();
        db.upsert_catalog_anime(
            &catalog_anime_full(6, "MidRun", &["Action"], "TV", Some(24), Some(72)),
            5,
        )
        .unwrap();
        db.upsert_catalog_anime(
            &catalog_anime_full(7, "LongRun", &["Action"], "TV", Some(500), Some(60)),
            6,
        )
        .unwrap();
        db.upsert_catalog_anime(
            &catalog_anime_full(8, "Unknown Length", &["Drama", "Action"], "TV", None, Some(50)),
            7,
        )
        .unwrap();
    }

    #[test]
    fn list_catalog_filtered_with_default_filter_matches_list_catalog() {
        let db = Db::open(":memory:").unwrap();
        seed_filter_catalog(&db);

        let default_filter = db.list_catalog_filtered(1, 100, &CatalogFilter::default()).unwrap();
        let legacy = db.list_catalog(1, 100).unwrap();
        assert_eq!(
            default_filter.iter().map(|a| a.id).collect::<Vec<_>>(),
            legacy.iter().map(|a| a.id).collect::<Vec<_>>()
        );
        assert_eq!(
            db.catalog_count_filtered(&CatalogFilter::default()).unwrap(),
            db.catalog_count().unwrap()
        );
    }

    #[test]
    fn list_catalog_filtered_search_is_case_insensitive_substring() {
        let db = Db::open(":memory:").unwrap();
        seed_filter_catalog(&db);

        let filter = CatalogFilter { search: Some("mONSteR".into()), ..Default::default() };
        let rows = db.list_catalog_filtered(1, 100, &filter).unwrap();
        assert_eq!(
            rows.iter().map(|a| a.title.as_str()).collect::<Vec<_>>(),
            vec!["Monster", "Monster Girl"]
        );
        assert_eq!(db.catalog_count_filtered(&filter).unwrap(), 2);
    }

    #[test]
    fn list_catalog_filtered_multi_genre_uses_and_semantics() {
        let db = Db::open(":memory:").unwrap();
        seed_filter_catalog(&db);

        // "Fullmetal Alchemist" (Drama+Action) and "Unknown Length" (Drama+Action)
        // have both genres; "Monster" (Drama+Mystery) and "ShortRun" (Action only)
        // have only one of the two and must be excluded.
        let filter = CatalogFilter {
            genres: vec!["Drama".into(), "Action".into()],
            ..Default::default()
        };
        let rows = db.list_catalog_filtered(1, 100, &filter).unwrap();
        let titles: Vec<&str> = rows.iter().map(|a| a.title.as_str()).collect();
        assert_eq!(titles, vec!["Fullmetal Alchemist", "Unknown Length"]);
        assert_eq!(db.catalog_count_filtered(&filter).unwrap(), 2);
    }

    #[test]
    fn list_catalog_filtered_format_exact_match() {
        let db = Db::open(":memory:").unwrap();
        seed_filter_catalog(&db);

        let filter = CatalogFilter { format: Some("MOVIE".into()), ..Default::default() };
        let rows = db.list_catalog_filtered(1, 100, &filter).unwrap();
        assert_eq!(rows.iter().map(|a| a.title.as_str()).collect::<Vec<_>>(), vec!["OnlyOne"]);
    }

    #[test]
    fn list_catalog_filtered_min_score_is_inclusive_floor() {
        let db = Db::open(":memory:").unwrap();
        seed_filter_catalog(&db);

        let filter = CatalogFilter { min_score: Some(70), ..Default::default() };
        let rows = db.list_catalog_filtered(1, 100, &filter).unwrap();
        // scores >= 70: Monster(90), Fullmetal(88), OnlyOne(70), MidRun(72)
        let mut titles: Vec<&str> = rows.iter().map(|a| a.title.as_str()).collect();
        titles.sort();
        assert_eq!(titles, vec!["Fullmetal Alchemist", "MidRun", "Monster", "OnlyOne"]);
    }

    #[test]
    fn list_catalog_filtered_episode_bucket_boundaries() {
        let db = Db::open(":memory:").unwrap();
        seed_filter_catalog(&db);

        let bucket = |b: &str| {
            let filter = CatalogFilter { episodes: Some(b.into()), ..Default::default() };
            let mut titles: Vec<String> = db
                .list_catalog_filtered(1, 100, &filter)
                .unwrap()
                .iter()
                .map(|a| a.title.clone())
                .collect();
            titles.sort();
            titles
        };

        assert_eq!(bucket("1"), vec!["OnlyOne".to_string()]);
        assert_eq!(bucket("2-12"), vec!["Monster Girl".to_string(), "ShortRun".to_string()]);
        assert_eq!(bucket("13-26"), vec!["MidRun".to_string()]);
        assert_eq!(bucket("27+"), vec!["Fullmetal Alchemist".to_string(), "LongRun".to_string(), "Monster".to_string()]);
        assert_eq!(bucket("unknown"), vec!["Unknown Length".to_string()]);
    }

    #[test]
    fn list_catalog_filtered_combines_all_filters() {
        let db = Db::open(":memory:").unwrap();
        seed_filter_catalog(&db);

        let filter = CatalogFilter {
            search: Some("run".into()),
            genres: vec!["Action".into()],
            format: Some("TV".into()),
            min_score: Some(60),
            episodes: Some("13-26".into()),
        };
        let rows = db.list_catalog_filtered(1, 100, &filter).unwrap();
        assert_eq!(rows.iter().map(|a| a.title.as_str()).collect::<Vec<_>>(), vec!["MidRun"]);
        assert_eq!(db.catalog_count_filtered(&filter).unwrap(), 1);
    }

    #[test]
    fn catalog_count_filtered_agrees_with_paginated_totals() {
        let db = Db::open(":memory:").unwrap();
        seed_filter_catalog(&db);

        let filter = CatalogFilter { genres: vec!["Action".into()], ..Default::default() };
        let total = db.catalog_count_filtered(&filter).unwrap();

        let mut seen = std::collections::HashSet::new();
        let mut page = 1;
        loop {
            let rows = db.list_catalog_filtered(page, 2, &filter).unwrap();
            if rows.is_empty() {
                break;
            }
            for r in &rows {
                seen.insert(r.id);
            }
            page += 1;
        }
        assert_eq!(seen.len() as i64, total);
    }
}
