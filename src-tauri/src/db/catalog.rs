use super::*;
use rusqlite::types::Value;

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
    /// Exact match against `anilist_catalog.studio`.
    pub studio: Option<String>,
}

impl Db {
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
            "INSERT INTO anilist_catalog(id, title, title_romaji, title_english, cover_url, format, episodes, average_score, popularity, url, sort_order, status, duration, studio, start_date)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)
             ON CONFLICT(id) DO UPDATE SET
                title=excluded.title, title_romaji=excluded.title_romaji, title_english=excluded.title_english,
                cover_url=excluded.cover_url, format=excluded.format,
                episodes=excluded.episodes, average_score=excluded.average_score,
                popularity=excluded.popularity, url=excluded.url, sort_order=excluded.sort_order,
                status=excluded.status, duration=excluded.duration, studio=excluded.studio,
                start_date=excluded.start_date",
            (
                anime.id, &anime.title, &anime.title_romaji, &anime.title_english, &anime.cover_url, &anime.format,
                anime.episodes, anime.average_score, anime.popularity, &anime.url, sort_order, &anime.status,
                anime.duration, &anime.studio, anime.start_date,
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

        let studio = filter.studio.as_deref().map(str::trim).filter(|s| !s.is_empty());
        if let Some(studio) = studio {
            conditions.push("studio = ?".to_string());
            params.push(Value::Text(studio.to_string()));
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
            "SELECT id, title, title_romaji, title_english, cover_url, format, episodes, average_score, popularity, url, status, duration, studio, start_date
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

    /// Distinct, non-null studio vocabulary from the synced catalog,
    /// alphabetical — drives the Catálogo filter bar's studio select.
    pub fn distinct_catalog_studios(&self) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT studio FROM anilist_catalog WHERE studio IS NOT NULL ORDER BY studio COLLATE NOCASE",
        )?;
        let studios = stmt
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(studios)
    }

    /// Normalized-title -> real premiere date (Unix ts) map built from every
    /// synced catalog row that has one, keyed under each of its
    /// title/title_romaji/title_english variants (`matching::normalize_title`,
    /// the same normalizer `engaged_series_titles`'s callers already rely on
    /// for matching site titles against synced catalog titles). Lets an
    /// airing-site row with no scraped episode data — most unfollowed ones,
    /// see `db::episodes::first_episode_dates`'s doc comment — still resolve
    /// a real "aired this season" verdict, entirely from the already-synced
    /// local AniList catalog. No site scraping involved. When a title has
    /// multiple variants mapping to different rows (unlikely, but the DB
    /// doesn't forbid it), the first one encountered wins — the same
    /// "good enough" trade-off `engaged_series_titles`-style matching
    /// already makes elsewhere.
    pub fn catalog_start_dates_by_normalized_title(&self) -> Result<std::collections::HashMap<String, i64>> {
        let mut stmt = self.conn.prepare(
            "SELECT title, title_romaji, title_english, start_date FROM anilist_catalog WHERE start_date IS NOT NULL",
        )?;
        let rows: Vec<(String, Option<String>, Option<String>, i64)> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut map = std::collections::HashMap::new();
        for (title, romaji, english, start_date) in rows {
            for variant in [Some(title), romaji, english].into_iter().flatten() {
                map.entry(crate::matching::normalize_title(&variant)).or_insert(start_date);
            }
        }
        Ok(map)
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
            status: r.get("status")?,
            duration: r.get("duration")?,
            studio: r.get("studio")?,
            start_date: r.get("start_date")?,
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

    /// Whether at least one synced catalog row has a non-NULL `status` —
    /// i.e. whether a sync has run since the `status` column was added.
    /// `hide_upcoming` silently excludes nothing while this is `false` (see
    /// `random_catalog_anime_in_genre`'s NULL-is-kept behavior), so the
    /// frontend uses this to warn the user their toggle won't take effect
    /// until they sync — see docs/superpowers/specs/2026-07-18-hide-upcoming-releases-design.md.
    pub fn has_synced_catalog_status(&self) -> Result<bool> {
        Ok(self
            .conn
            .query_row("SELECT EXISTS(SELECT 1 FROM anilist_catalog WHERE status IS NOT NULL)", [], |r| {
                r.get::<_, bool>(0)
            })?)
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
    /// `excluded_norm_titles` — normalized (`matching::normalize_title`)
    /// titles of series the user has already engaged with (followed,
    /// wanted, discarded, or marked watched-externally; see
    /// `engaged_series_titles`), scoped to the caller's source. Needed
    /// because the pre-existing `anilist_id NOT IN series` clause below only
    /// catches engaged series that got linked to a real AniList id — and
    /// followed *site*-scraped series almost never do (linking is on-demand,
    /// rarely triggered), so without this a followed show with the exact
    /// same title as a catalog entry keeps getting re-offered by the deck
    /// (see docs/superpowers/specs/2026-07-12-discover-exclude-followed-design.md).
    ///
    /// Rather than `ORDER BY RANDOM() LIMIT 1` and rejecting only that one
    /// row (which would silently starve the deck for any genre where the
    /// single random pick happens to be engaged), this pulls a
    /// `BATCH_SIZE`-row random batch and keeps every `survivor` — a
    /// candidate whose title/`title_romaji`/`title_english` (skipping NULLs)
    /// all normalize to something outside `excluded_norm_titles`. Kept as an
    /// explicit `survivors` vec so the taste-scoring pass
    /// (`recommend::pick_recommended`) can score the whole batch and pick the
    /// best-fitting one instead of just the first, without touching the
    /// batch-fetch or exclusion logic above it — see
    /// docs/superpowers/specs/2026-07-12-discover-recommendation-engine-design.md.
    /// Genres are backfilled per-survivor (not just for the final pick)
    /// because `score_candidate`'s secondary-genre-overlap term needs each
    /// candidate's full genre list — the batch query only joins on the one
    /// `genre` column being filtered on, so `row_to_catalog_anime` always
    /// returns an empty `genres` vec. `genre_affinity`/`format_affinity` come
    /// from the caller (`commands::discover_catalog_card`) — pass empty maps
    /// for the pre-recommendation-engine behavior (uniform pick over the
    /// batch, still exclusion/ban-correct).
    ///
    /// `recommended` selects the final-pick strategy, not the batch/ban/
    /// quality-floor/exclusion logic above it (that stays identical either
    /// way): `true` scores the survivors with `recommend::pick_recommended`
    /// (unchanged prior behavior); `false` bypasses scoring and returns
    /// `survivors.into_iter().next()` — the batch is already
    /// `ORDER BY RANDOM()`, so the first survivor is a uniform random pick.
    /// Passing empty affinity maps alone is NOT equivalent to `false`: an
    /// empty map still leaves `score_candidate`'s quality term active, which
    /// biases toward higher `average_score` rather than being genuinely
    /// uniform — see docs/superpowers/specs/2026-07-13-discover-recommendation-toggle-design.md.
    pub fn random_catalog_anime_in_genre(
        &self,
        genre: &str,
        banned_formats: &[String],
        excluded_norm_titles: &std::collections::HashSet<String>,
        genre_affinity: &std::collections::HashMap<String, f64>,
        format_affinity: &std::collections::HashMap<String, f64>,
        recommended: bool,
        hide_upcoming: bool,
    ) -> Result<Option<crate::anilist::CatalogAnime>> {
        const MIN_POPULARITY: i64 = 500;
        // Raised from 40: a random 40-title sample out of a genre that can
        // hold thousands of catalog rows too often missed the actual
        // best-scoring candidates before `recommend::pick_recommended` ever
        // saw them. 150 stays cheap (one local, indexed SQLite query) while
        // giving the scorer a meaningfully wider pool to rank.
        const BATCH_SIZE: i64 = 150;
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
        // `status IS NULL` is deliberately included on the "keep" side: rows
        // synced before the `status` column existed (or an unsynced/partial
        // sync) stay eligible rather than vanishing from the deck the moment
        // this toggle is flipped on.
        let upcoming_clause =
            if hide_upcoming { "AND (c.status IS NULL OR c.status != 'NOT_YET_RELEASED')" } else { "" };
        let sql = format!(
            "SELECT c.id, c.title, c.title_romaji, c.title_english, c.cover_url, c.format, c.episodes, c.average_score, c.popularity, c.url, c.status, c.duration, c.studio, c.start_date
             FROM anilist_catalog c
             JOIN anilist_catalog_genres g ON g.anilist_id = c.id
             WHERE g.genre = ?
               AND c.format IN ({placeholders})
               AND c.popularity >= ?
               AND c.id NOT IN (SELECT anilist_id FROM series WHERE anilist_id IS NOT NULL)
               {upcoming_clause}
             ORDER BY RANDOM() LIMIT {BATCH_SIZE}"
        );
        let mut params: Vec<Value> = vec![Value::Text(genre.to_string())];
        for f in &allowed {
            params.push(Value::Text((*f).to_string()));
        }
        params.push(Value::Integer(MIN_POPULARITY));
        let mut stmt = self.conn.prepare(&sql)?;
        let batch: Vec<crate::anilist::CatalogAnime> = stmt
            .query_map(rusqlite::params_from_iter(params.iter()), Self::row_to_catalog_anime)?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        let is_engaged_by_title = |anime: &crate::anilist::CatalogAnime| -> bool {
            [Some(&anime.title), anime.title_romaji.as_ref(), anime.title_english.as_ref()]
                .into_iter()
                .flatten()
                .any(|t| excluded_norm_titles.contains(&crate::matching::normalize_title(t)))
        };
        let mut survivors: Vec<crate::anilist::CatalogAnime> =
            batch.into_iter().filter(|a| !is_engaged_by_title(a)).collect();
        for anime in &mut survivors {
            anime.genres = self.list_catalog_genres(anime.id)?;
        }

        if recommended {
            let now_unix = chrono::Utc::now().timestamp();
            Ok(crate::recommend::pick_recommended(survivors, genre_affinity, format_affinity, genre, now_unix))
        } else {
            Ok(survivors.into_iter().next())
        }
    }

    /// Normalized-title exclusion set input: titles of `series` rows the
    /// user has already decided on for this source — followed, "want",
    /// "discarded", or watched-externally. See
    /// `random_catalog_anime_in_genre`'s doc comment for why this is needed
    /// alongside (not instead of) its `anilist_id NOT IN series` clause.
    /// Returns raw titles (not yet normalized) — callers normalize via
    /// `matching::normalize_title` when building the exclusion set, keeping
    /// this function a pure DB read.
    pub fn engaged_series_titles(&self, source_id: i64) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT title FROM series
             WHERE source_id=?1
               AND (followed=1 OR watched_externally=1 OR backlog_status IN ('want','discarded'))",
        )?;
        let titles = stmt.query_map([source_id], |r| r.get::<_, String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(titles)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_support::*;

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
    fn has_synced_catalog_status_false_when_empty_or_all_null_true_once_one_row_has_status() {
        let db = Db::open(":memory:").unwrap();
        assert_eq!(db.has_synced_catalog_status().unwrap(), false);

        db.upsert_catalog_anime(&catalog_anime_with_popularity(1, "Unsynced", &["Drama"], Some(1000)), 0).unwrap();
        assert_eq!(db.has_synced_catalog_status().unwrap(), false, "a NULL-status row alone must not count as synced");

        let mut synced = catalog_anime_with_popularity(2, "Synced", &["Drama"], Some(1000));
        synced.status = Some("RELEASING".into());
        db.upsert_catalog_anime(&synced, 1).unwrap();
        assert_eq!(db.has_synced_catalog_status().unwrap(), true);
    }

    #[test]
    fn random_catalog_anime_in_genre_none_when_empty_some_when_populated() {
        let db = Db::open(":memory:").unwrap();
        assert!(db.random_catalog_anime_in_genre("Drama", &[], &std::collections::HashSet::new(), &std::collections::HashMap::new(), &std::collections::HashMap::new(), true, false).unwrap().is_none());
        db.upsert_catalog_anime(
            &catalog_anime_with_popularity(1, "Only", &["Drama"], Some(1000)),
            0,
        )
        .unwrap();
        let picked = db.random_catalog_anime_in_genre("Drama", &[], &std::collections::HashSet::new(), &std::collections::HashMap::new(), &std::collections::HashMap::new(), true, false).unwrap().unwrap();
        assert_eq!(picked.id, 1);
        assert_eq!(picked.genres, vec!["Drama".to_string()]);
    }

    #[test]
    fn random_catalog_anime_in_genre_only_matches_requested_genre() {
        let db = Db::open(":memory:").unwrap();
        db.upsert_catalog_anime(&catalog_anime_with_popularity(1, "Dramatic", &["Drama"], Some(1000)), 0).unwrap();
        db.upsert_catalog_anime(&catalog_anime_with_popularity(2, "Actiony", &["Action"], Some(1000)), 1).unwrap();
        let picked = db.random_catalog_anime_in_genre("Action", &[], &std::collections::HashSet::new(), &std::collections::HashMap::new(), &std::collections::HashMap::new(), true, false).unwrap().unwrap();
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

        let picked = db.random_catalog_anime_in_genre("Drama", &[], &std::collections::HashSet::new(), &std::collections::HashMap::new(), &std::collections::HashMap::new(), true, false).unwrap().unwrap();
        assert_eq!(picked.title, "Qualifies");
    }

    #[test]
    fn random_catalog_anime_in_genre_excludes_not_yet_released_when_hide_upcoming_true() {
        let db = Db::open(":memory:").unwrap();
        let mut upcoming = catalog_anime_with_popularity(1, "Upcoming", &["Drama"], Some(1000));
        upcoming.status = Some("NOT_YET_RELEASED".into());
        db.upsert_catalog_anime(&upcoming, 0).unwrap();
        db.upsert_catalog_anime(&catalog_anime_with_popularity(2, "OutNow", &["Drama"], Some(1000)), 1).unwrap();

        let picked = db
            .random_catalog_anime_in_genre("Drama", &[], &std::collections::HashSet::new(), &std::collections::HashMap::new(), &std::collections::HashMap::new(), true, true)
            .unwrap()
            .unwrap();
        assert_eq!(picked.title, "OutNow");
    }

    #[test]
    fn random_catalog_anime_in_genre_includes_null_status_when_hide_upcoming_true() {
        // Unsynced rows (status still NULL, e.g. before the first resync
        // after this column existed) must NOT vanish from the deck just
        // because the toggle is on — only a confirmed NOT_YET_RELEASED
        // status is excluded.
        let db = Db::open(":memory:").unwrap();
        db.upsert_catalog_anime(&catalog_anime_with_popularity(1, "UnsyncedStatus", &["Drama"], Some(1000)), 0).unwrap();

        let picked = db
            .random_catalog_anime_in_genre("Drama", &[], &std::collections::HashSet::new(), &std::collections::HashMap::new(), &std::collections::HashMap::new(), true, true)
            .unwrap()
            .unwrap();
        assert_eq!(picked.title, "UnsyncedStatus");
    }

    #[test]
    fn random_catalog_anime_in_genre_includes_upcoming_when_hide_upcoming_false() {
        let db = Db::open(":memory:").unwrap();
        let mut upcoming = catalog_anime_with_popularity(1, "Upcoming", &["Drama"], Some(1000));
        upcoming.status = Some("NOT_YET_RELEASED".into());
        db.upsert_catalog_anime(&upcoming, 0).unwrap();

        let picked = db
            .random_catalog_anime_in_genre("Drama", &[], &std::collections::HashSet::new(), &std::collections::HashMap::new(), &std::collections::HashMap::new(), true, false)
            .unwrap()
            .unwrap();
        assert_eq!(picked.title, "Upcoming");
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
            let picked = db.random_catalog_anime_in_genre("Drama", &[], &std::collections::HashSet::new(), &std::collections::HashMap::new(), &std::collections::HashMap::new(), true, false).unwrap().unwrap();
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

        assert!(db.random_catalog_anime_in_genre("Drama", &[], &std::collections::HashSet::new(), &std::collections::HashMap::new(), &std::collections::HashMap::new(), true, false).unwrap().is_none());
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
            let picked = db
                .random_catalog_anime_in_genre("Drama", &["MOVIE".to_string()], &std::collections::HashSet::new(), &std::collections::HashMap::new(), &std::collections::HashMap::new(), true, false)
                .unwrap()
                .unwrap();
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
        assert!(db
            .random_catalog_anime_in_genre("Drama", &all_banned, &std::collections::HashSet::new(), &std::collections::HashMap::new(), &std::collections::HashMap::new(), true, false)
            .unwrap()
            .is_none());
    }

    #[test]
    fn random_catalog_anime_in_genre_excludes_engaged_normalized_title() {
        // Root cause (docs/superpowers/specs/2026-07-12-discover-exclude-followed-design.md):
        // followed site series never get an anilist_id (linking is on-demand
        // and rare), so the pre-existing `anilist_id NOT IN series` clause
        // catches ~zero engaged series. The deck must also exclude by
        // normalized title so a followed site row with no anilist_id still
        // blocks the same-titled catalog entry.
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
        db.upsert_catalog_anime(&catalog_anime_with_popularity(1, "Overlord IV", &["Fantasy"], Some(1000)), 0)
            .unwrap();
        // Control: an un-followed catalog title in the same genre must stay
        // returnable, proving the exclusion is title-specific, not blanket.
        db.upsert_catalog_anime(&catalog_anime_with_popularity(2, "Some Other Show", &["Fantasy"], Some(1000)), 1)
            .unwrap();

        let followed = crate::models::Series {
            id: 0,
            slug: "overlord-iv".into(),
            title: "Overlord IV".into(),
            url: "https://example.com/tv/overlord-iv".into(),
            cover_url: None,
            is_airing: false,
            followed: true,
            next_episode_at: None,
            site_episode_count: None,
        };
        let sid = db.upsert_series(src, &followed).unwrap();
        db.set_followed(sid, true).unwrap();
        // Deliberately no set_anilist_id: this is the exact bug scenario —
        // followed=1, anilist_id NULL.

        let excluded: std::collections::HashSet<String> =
            db.engaged_series_titles(src).unwrap().iter().map(|t| crate::matching::normalize_title(t)).collect();
        assert!(excluded.contains(&crate::matching::normalize_title("Overlord IV")));

        for _ in 0..10 {
            let picked = db.random_catalog_anime_in_genre("Fantasy", &[], &excluded, &std::collections::HashMap::new(), &std::collections::HashMap::new(), true, false).unwrap().unwrap();
            assert_eq!(picked.title, "Some Other Show");
        }
    }

    #[test]
    fn random_catalog_anime_in_genre_returns_none_when_only_candidate_is_engaged_by_title() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
        db.upsert_catalog_anime(&catalog_anime_with_popularity(1, "Overlord IV", &["Fantasy"], Some(1000)), 0)
            .unwrap();
        let followed = crate::models::Series {
            id: 0,
            slug: "overlord-iv".into(),
            title: "Overlord IV".into(),
            url: "https://example.com/tv/overlord-iv".into(),
            cover_url: None,
            is_airing: false,
            followed: true,
            next_episode_at: None,
            site_episode_count: None,
        };
        let sid = db.upsert_series(src, &followed).unwrap();
        db.set_followed(sid, true).unwrap();

        let excluded: std::collections::HashSet<String> =
            db.engaged_series_titles(src).unwrap().iter().map(|t| crate::matching::normalize_title(t)).collect();
        assert!(db.random_catalog_anime_in_genre("Fantasy", &[], &excluded, &std::collections::HashMap::new(), &std::collections::HashMap::new(), true, false).unwrap().is_none());
    }

    #[test]
    fn random_catalog_anime_in_genre_end_to_end_biases_toward_secondary_genre_affinity() {
        // Integration check that the recommendation engine is actually wired
        // through the DB layer, not just correct in isolation
        // (`recommend::tests` covers the pure scorer). Both candidates match
        // the requested outer genre "Drama" (excluded from scoring — see
        // `score_candidate`'s doc comment), so only a real backfill of each
        // survivor's FULL genre list (via `list_catalog_genres`, not just
        // the queried column) lets the secondary-genre overlap with
        // "Fantasy" actually surface and bias the pick.
        let db = Db::open(":memory:").unwrap();
        db.upsert_catalog_anime(
            &catalog_anime_with_popularity(1, "TasteMatch", &["Drama", "Fantasy"], Some(1000)),
            0,
        )
        .unwrap();
        db.upsert_catalog_anime(&catalog_anime_with_popularity(2, "NoMatch", &["Drama"], Some(1000)), 1).unwrap();

        let mut genre_affinity = std::collections::HashMap::new();
        genre_affinity.insert("Fantasy".to_string(), 10.0);
        let format_affinity = std::collections::HashMap::new();

        let mut taste_match_wins = 0;
        for _ in 0..30 {
            let picked = db
                .random_catalog_anime_in_genre(
                    "Drama",
                    &[],
                    &std::collections::HashSet::new(),
                    &genre_affinity,
                    &format_affinity,
                    true,
                    false,
                )
                .unwrap()
                .unwrap();
            if picked.title == "TasteMatch" {
                taste_match_wins += 1;
            }
        }
        assert!(
            taste_match_wins >= 25,
            "expected the secondary-genre-matching candidate to dominate the weighted pick, won {taste_match_wins}/30"
        );
    }

    #[test]
    fn random_catalog_anime_in_genre_recommended_false_ignores_affinity_bias() {
        // Mirrors `..._end_to_end_biases_toward_secondary_genre_affinity` but
        // with recommended=false: the same heavy Fantasy affinity that
        // dominates the recommended-mode pick must NOT bias the random-mode
        // pick — recommended=false bypasses `pick_recommended` entirely and
        // just returns the first (already `ORDER BY RANDOM()`) survivor.
        let db = Db::open(":memory:").unwrap();
        db.upsert_catalog_anime(
            &catalog_anime_with_popularity(1, "TasteMatch", &["Drama", "Fantasy"], Some(1000)),
            0,
        )
        .unwrap();
        db.upsert_catalog_anime(&catalog_anime_with_popularity(2, "NoMatch", &["Drama"], Some(1000)), 1).unwrap();

        let mut genre_affinity = std::collections::HashMap::new();
        genre_affinity.insert("Fantasy".to_string(), 10.0);
        let format_affinity = std::collections::HashMap::new();

        let mut taste_match_wins = 0;
        for _ in 0..30 {
            let picked = db
                .random_catalog_anime_in_genre(
                    "Drama",
                    &[],
                    &std::collections::HashSet::new(),
                    &genre_affinity,
                    &format_affinity,
                    false,
                    false,
                )
                .unwrap()
                .unwrap();
            if picked.title == "TasteMatch" {
                taste_match_wins += 1;
            }
        }
        // Wide bounds (should land near 15/30) purely to avoid flakiness —
        // the point is it must NOT dominate like the recommended-mode test
        // (which asserts >= 25/30).
        assert!(
            (5..=25).contains(&taste_match_wins),
            "expected roughly even split ignoring affinity, got {taste_match_wins}/30 for TasteMatch"
        );
    }

    #[test]
    fn random_catalog_anime_in_genre_recommended_false_still_excludes_engaged_titles() {
        // recommended=false must skip scoring, not skip exclusion — the
        // batch-fetch/ban/quality-floor/exclusion logic is shared regardless
        // of mode.
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
        db.upsert_catalog_anime(&catalog_anime_with_popularity(1, "Overlord IV", &["Fantasy"], Some(1000)), 0)
            .unwrap();
        db.upsert_catalog_anime(&catalog_anime_with_popularity(2, "Some Other Show", &["Fantasy"], Some(1000)), 1)
            .unwrap();

        let followed = crate::models::Series {
            id: 0,
            slug: "overlord-iv".into(),
            title: "Overlord IV".into(),
            url: "https://example.com/tv/overlord-iv".into(),
            cover_url: None,
            is_airing: false,
            followed: true,
            next_episode_at: None,
            site_episode_count: None,
        };
        let sid = db.upsert_series(src, &followed).unwrap();
        db.set_followed(sid, true).unwrap();

        let excluded: std::collections::HashSet<String> =
            db.engaged_series_titles(src).unwrap().iter().map(|t| crate::matching::normalize_title(t)).collect();

        for _ in 0..10 {
            let picked = db
                .random_catalog_anime_in_genre(
                    "Fantasy",
                    &[],
                    &excluded,
                    &std::collections::HashMap::new(),
                    &std::collections::HashMap::new(),
                    false,
                    false,
                )
                .unwrap()
                .unwrap();
            assert_eq!(picked.title, "Some Other Show");
        }
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
    fn list_catalog_filtered_studio_exact_match() {
        let db = Db::open(":memory:").unwrap();
        let mut a = catalog_anime_with_popularity(1, "MadeByA", &["Drama"], Some(1000));
        a.studio = Some("Studio A".into());
        db.upsert_catalog_anime(&a, 0).unwrap();
        let mut b = catalog_anime_with_popularity(2, "MadeByB", &["Drama"], Some(1000));
        b.studio = Some("Studio B".into());
        db.upsert_catalog_anime(&b, 1).unwrap();
        // No studio at all — must not match either filter value.
        db.upsert_catalog_anime(&catalog_anime_with_popularity(3, "NoStudio", &["Drama"], Some(1000)), 2).unwrap();

        let filter = CatalogFilter { studio: Some("Studio A".into()), ..Default::default() };
        let rows = db.list_catalog_filtered(1, 100, &filter).unwrap();
        assert_eq!(rows.iter().map(|a| a.title.as_str()).collect::<Vec<_>>(), vec!["MadeByA"]);
        assert_eq!(db.catalog_count_filtered(&filter).unwrap(), 1);
    }

    #[test]
    fn distinct_catalog_studios_returns_alphabetical_non_null() {
        let db = Db::open(":memory:").unwrap();
        let mut a = catalog_anime_with_popularity(1, "First", &[], Some(1000));
        a.studio = Some("Zeta Studio".into());
        db.upsert_catalog_anime(&a, 0).unwrap();
        let mut b = catalog_anime_with_popularity(2, "Second", &[], Some(1000));
        b.studio = Some("Alpha Studio".into());
        db.upsert_catalog_anime(&b, 1).unwrap();
        // No studio — must not produce a NULL entry in the vocabulary.
        db.upsert_catalog_anime(&catalog_anime_with_popularity(3, "Third", &[], Some(1000)), 2).unwrap();

        let studios = db.distinct_catalog_studios().unwrap();
        assert_eq!(studios, vec!["Alpha Studio".to_string(), "Zeta Studio".to_string()]);
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
            ..Default::default()
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
            status: None,
            duration: None,
            studio: None,
            start_date: None,
        };
        db.upsert_catalog_anime(&anime, 0).unwrap();

        let (title, romaji, english) = db.get_catalog_titles(42).unwrap().unwrap();
        assert_eq!(title, "Attack on Titan");
        assert_eq!(romaji.as_deref(), Some("Shingeki no Kyojin"));
        assert_eq!(english.as_deref(), Some("Attack on Titan"));
    }

    #[test]
    fn upsert_catalog_anime_round_trips_duration() {
        let db = Db::open(":memory:").unwrap();

        // A row with a real synced duration.
        let with_duration = crate::anilist::CatalogAnime {
            id: 43,
            title: "Timed Show".into(),
            title_romaji: None,
            title_english: None,
            cover_url: None,
            format: Some("TV".into()),
            genres: vec![],
            episodes: Some(12),
            average_score: None,
            popularity: None,
            url: "https://anilist.co/anime/43".into(),
            status: None,
            duration: Some(23),
            studio: None,
            start_date: None,
        };
        db.upsert_catalog_anime(&with_duration, 0).unwrap();

        // A row that never sets duration (mirrors a pre-existing/unsynced
        // row) — must default to None, not error or coerce to 0.
        let without_duration = crate::anilist::CatalogAnime {
            id: 44,
            title: "Undated Show".into(),
            title_romaji: None,
            title_english: None,
            cover_url: None,
            format: Some("TV".into()),
            genres: vec![],
            episodes: Some(12),
            average_score: None,
            popularity: None,
            url: "https://anilist.co/anime/44".into(),
            status: None,
            duration: None,
            studio: None,
            start_date: None,
        };
        db.upsert_catalog_anime(&without_duration, 1).unwrap();

        let page = db.list_catalog(1, 10).unwrap();
        let read_with = page.iter().find(|a| a.id == 43).unwrap();
        assert_eq!(read_with.duration, Some(23), "real synced duration must round-trip");
        let read_without = page.iter().find(|a| a.id == 44).unwrap();
        assert_eq!(read_without.duration, None, "unset duration must read back as None, not 0");
    }

    #[test]
    fn upsert_catalog_anime_round_trips_studio() {
        let db = Db::open(":memory:").unwrap();

        // A row with a real synced studio.
        let with_studio = crate::anilist::CatalogAnime {
            id: 45,
            title: "Studio Show".into(),
            title_romaji: None,
            title_english: None,
            cover_url: None,
            format: Some("TV".into()),
            genres: vec![],
            episodes: Some(12),
            average_score: None,
            popularity: None,
            url: "https://anilist.co/anime/45".into(),
            status: None,
            duration: None,
            studio: Some("Studio Ghibli".into()),
            start_date: None,
        };
        db.upsert_catalog_anime(&with_studio, 0).unwrap();

        // A row that never sets studio (mirrors a pre-existing/unsynced row,
        // or a title AniList credits no studio for) — must default to None,
        // not error or coerce to an empty string.
        let without_studio = crate::anilist::CatalogAnime {
            id: 46,
            title: "No Studio Show".into(),
            title_romaji: None,
            title_english: None,
            cover_url: None,
            format: Some("TV".into()),
            genres: vec![],
            episodes: Some(12),
            average_score: None,
            popularity: None,
            url: "https://anilist.co/anime/46".into(),
            status: None,
            duration: None,
            studio: None,
            start_date: None,
        };
        db.upsert_catalog_anime(&without_studio, 1).unwrap();

        let page = db.list_catalog(1, 10).unwrap();
        let read_with = page.iter().find(|a| a.id == 45).unwrap();
        assert_eq!(read_with.studio.as_deref(), Some("Studio Ghibli"), "real synced studio must round-trip");
        let read_without = page.iter().find(|a| a.id == 46).unwrap();
        assert_eq!(read_without.studio, None, "unset studio must read back as None, not empty string");
    }

    #[test]
    fn upsert_catalog_anime_round_trips_start_date() {
        let db = Db::open(":memory:").unwrap();

        let with_date = crate::anilist::CatalogAnime {
            id: 47,
            title: "Dated Show".into(),
            title_romaji: None,
            title_english: None,
            cover_url: None,
            format: Some("TV".into()),
            genres: vec![],
            episodes: Some(12),
            average_score: None,
            popularity: None,
            url: "https://anilist.co/anime/47".into(),
            status: None,
            duration: None,
            studio: None,
            start_date: Some(1_776_211_200), // 2026-04-15T00:00:00Z
        };
        db.upsert_catalog_anime(&with_date, 0).unwrap();

        let without_date = crate::anilist::CatalogAnime {
            id: 48,
            title: "Undated Show".into(),
            title_romaji: None,
            title_english: None,
            cover_url: None,
            format: Some("TV".into()),
            genres: vec![],
            episodes: Some(12),
            average_score: None,
            popularity: None,
            url: "https://anilist.co/anime/48".into(),
            status: None,
            duration: None,
            studio: None,
            start_date: None,
        };
        db.upsert_catalog_anime(&without_date, 1).unwrap();

        let page = db.list_catalog(1, 10).unwrap();
        let read_with = page.iter().find(|a| a.id == 47).unwrap();
        assert_eq!(read_with.start_date, Some(1_776_211_200), "real synced start_date must round-trip");
        let read_without = page.iter().find(|a| a.id == 48).unwrap();
        assert_eq!(read_without.start_date, None, "unset start_date must read back as None, not 0");
    }

    #[test]
    fn catalog_start_dates_by_normalized_title_covers_all_title_variants_and_skips_null() {
        let db = Db::open(":memory:").unwrap();

        // Matched under its romaji title on the (hypothetical) scraped site,
        // even though `title` itself collapsed to English.
        db.upsert_catalog_anime(
            &crate::anilist::CatalogAnime {
                id: 50, title: "Attack on Titan".into(), title_romaji: Some("Shingeki no Kyojin".into()),
                title_english: Some("Attack on Titan".into()), cover_url: None, format: Some("TV".into()),
                genres: vec![], episodes: Some(25), average_score: None, popularity: None,
                url: "https://anilist.co/anime/50".into(), status: None, duration: None, studio: None,
                start_date: Some(1_776_211_200),
            },
            0,
        ).unwrap();

        // No start_date synced yet — must be entirely absent from the map,
        // not present with a None/0 value.
        db.upsert_catalog_anime(
            &crate::anilist::CatalogAnime {
                id: 51, title: "Unsynced Show".into(), title_romaji: None, title_english: None,
                cover_url: None, format: Some("TV".into()), genres: vec![], episodes: None,
                average_score: None, popularity: None, url: "https://anilist.co/anime/51".into(),
                status: None, duration: None, studio: None, start_date: None,
            },
            1,
        ).unwrap();

        let map = db.catalog_start_dates_by_normalized_title().unwrap();
        assert_eq!(map.get(&crate::matching::normalize_title("Attack on Titan")), Some(&1_776_211_200));
        assert_eq!(map.get(&crate::matching::normalize_title("Shingeki no Kyojin")), Some(&1_776_211_200));
        assert_eq!(map.get(&crate::matching::normalize_title("Unsynced Show")), None);
        assert_eq!(map.len(), 2, "one entry per distinct title/romaji/english variant with a real start_date");
    }
}
