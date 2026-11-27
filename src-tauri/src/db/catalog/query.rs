use super::*;

impl Db {
    /// `(title, title_romaji, title_english)` for a synced catalog entry —
    /// `link_catalog_series` tries `title_romaji.unwrap_or(title)` first,
    /// then `title_english` if that fails to match anything on the site.
    /// Returns `(display_title, romaji, english)`.
    #[allow(clippy::type_complexity)]
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
            conditions.push(
                "(title LIKE '%' || ? || '%' COLLATE NOCASE \
                 OR (title_romaji IS NOT NULL AND title_romaji LIKE '%' || ? || '%' COLLATE NOCASE) \
                 OR (title_english IS NOT NULL AND title_english LIKE '%' || ? || '%' COLLATE NOCASE) \
                 OR id IN (SELECT anilist_id FROM anilist_catalog_tags WHERE tag LIKE '%' || ? || '%' COLLATE NOCASE) \
                 OR id IN (SELECT anilist_id FROM anilist_catalog_genres WHERE genre LIKE '%' || ? || '%' COLLATE NOCASE) \
                 OR id IN (SELECT anilist_id FROM anilist_catalog_synonyms WHERE synonym LIKE '%' || ? || '%' COLLATE NOCASE))"
                    .to_string(),
            );
            params.push(Value::Text(search.to_string()));
            params.push(Value::Text(search.to_string()));
            params.push(Value::Text(search.to_string()));
            params.push(Value::Text(search.to_string()));
            params.push(Value::Text(search.to_string()));
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

        if let Some(max_score) = filter.max_score {
            conditions.push("average_score <= ?".to_string());
            params.push(Value::Integer(max_score));
        }

        if let Some(min_date) = filter.min_start_date {
            conditions.push("start_date >= ?".to_string());
            params.push(Value::Integer(min_date));
        }

        if let Some(max_date) = filter.max_start_date {
            conditions.push("start_date <= ?".to_string());
            params.push(Value::Integer(max_date));
        }

        let status = filter.status.as_deref().map(str::trim).filter(|s| !s.is_empty());
        if let Some(status) = status {
            let upper = status.to_ascii_uppercase();
            if matches!(upper.as_str(), "RELEASING" | "FINISHED" | "NOT_YET_RELEASED" | "CANCELLED" | "HIATUS") {
                conditions.push("status = ?".to_string());
                params.push(Value::Text(upper));
            }
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
            for genre in &genres {
                conditions.push(
                    "(id IN (SELECT anilist_id FROM anilist_catalog_genres WHERE genre = ? COLLATE NOCASE) \
                      OR id IN (SELECT anilist_id FROM anilist_catalog_tags WHERE tag = ? COLLATE NOCASE))"
                        .to_string(),
                );
                params.push(Value::Text((*genre).to_string()));
                params.push(Value::Text((*genre).to_string()));
            }
        }

        if filter.hide_watched {
            // Every comparison is `x NOT IN (uncorrelated subquery)`, which
            // SQLite evaluates once into an ephemeral index. The previous
            // form joined every catalog row against every series row on three
            // OR-ed title comparisons (22k x 6k) and took ~18 s while holding
            // the DB mutex, freezing the UI on every filter change.
            const WATCHED: &str = "FROM series s WHERE (s.watched_externally = 1                  OR s.id IN (SELECT series_id FROM episodes WHERE seen = 1))";
            conditions.push(format!(
                "(id NOT IN (SELECT s.anilist_id {WATCHED} AND s.anilist_id IS NOT NULL)                   AND title COLLATE NOCASE NOT IN (SELECT s.title {WATCHED})                   AND (title_romaji IS NULL OR title_romaji COLLATE NOCASE NOT IN (SELECT s.title {WATCHED}))                   AND (title_english IS NULL OR title_english COLLATE NOCASE NOT IN (SELECT s.title {WATCHED})))"
            ));
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
        let order_by = match filter.sort.as_deref().map(str::trim) {
            Some("score_desc") => "average_score DESC NULLS LAST, id ASC",
            Some("score_asc") => "average_score ASC NULLS LAST, id ASC",
            Some("newest") => "start_date DESC NULLS LAST, id ASC",
            Some("oldest") => "start_date ASC NULLS LAST, id ASC",
            Some("title") => "title COLLATE NOCASE ASC, id ASC",
            _ => "popularity DESC NULLS LAST, id ASC",
        };
        let sql = format!(
            "SELECT id, title, title_romaji, title_english, cover_url, banner_url, format, episodes, average_score, popularity, url, status, duration, studio, start_date, next_airing_at, next_episode
             FROM anilist_catalog WHERE {where_sql}
             ORDER BY {order_by} LIMIT ? OFFSET ?"
        );
        params.push(Value::Integer(per_page));
        params.push(Value::Integer(offset));

        let mut stmt = self.conn.prepare(&sql)?;
        let mut rows = stmt
            .query_map(rusqlite::params_from_iter(params.iter()), Self::row_to_catalog_anime)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for anime in &mut rows {
            anime.genres = self.list_catalog_genres(anime.id)?;
            anime.tags = self.list_catalog_tags(anime.id)?;
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

    /// Distinct genre and major thematic tag vocabulary from the synced catalog,
    /// alphabetical — drives the Catálogo filter bar's genre chips.
    pub fn distinct_catalog_genres(&self) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT genre FROM (
                SELECT genre FROM anilist_catalog_genres
                UNION
                SELECT tag AS genre FROM anilist_catalog_tags
                WHERE rank >= 50 AND tag IN (
                    'Isekai', 'Magic', 'School', 'Super Power', 'Martial Arts', 'Military',
                    'Historical', 'Parody', 'Space', 'Vampire', 'Cyberpunk', 'Time Travel',
                    'Reincarnation', 'Virtual World', 'Cultivation', 'Post-Apocalyptic',
                    'Idol', 'Gore', 'Mythology', 'Detective', 'Survival', 'Video Games'
                )
            ) ORDER BY genre COLLATE NOCASE",
        )?;
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
    #[cfg(test)] // superseded in production; kept as a test oracle
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

    /// Normalized-title -> banner_url map from every synced catalog row that has one.
    #[cfg(test)] // superseded in production; kept as a test oracle
    pub fn catalog_banners_by_normalized_title(&self) -> Result<std::collections::HashMap<String, String>> {
        let mut stmt = self.conn.prepare(
            "SELECT title, title_romaji, title_english, banner_url FROM anilist_catalog WHERE banner_url IS NOT NULL AND banner_url != ''",
        )?;
        let rows: Vec<(String, Option<String>, Option<String>, String)> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut map = std::collections::HashMap::new();
        for (title, romaji, english, banner_url) in rows {
            for variant in [Some(title), romaji, english].into_iter().flatten() {
                map.entry(crate::matching::normalize_title(&variant)).or_insert_with(|| banner_url.clone());
            }
        }
        Ok(map)
    }

    /// Query the banner URL for a specific series row, checking series.banner_url
    /// first, then falling back to linked anilist_catalog.banner_url.
    pub fn get_series_banner(&self, series_id: i64) -> Result<Option<String>> {
        let opt: Option<Option<String>> = self
            .conn
            .query_row(
                "SELECT COALESCE(s.banner_url, c.banner_url)
                 FROM series s
                 LEFT JOIN anilist_catalog c ON c.id = s.anilist_id
                 WHERE s.id = ?1",
                [series_id],
                |r| r.get(0),
            )
            .optional()?;
        Ok(opt.flatten())
    }

    /// Normalized-title -> average_score map from every synced catalog row that has one.
    #[cfg(test)] // superseded in production; kept as a test oracle
    pub fn catalog_scores_by_normalized_title(&self) -> Result<std::collections::HashMap<String, f64>> {
        let mut stmt = self.conn.prepare(
            "SELECT title, title_romaji, title_english, average_score FROM anilist_catalog WHERE average_score IS NOT NULL",
        )?;
        let rows: Vec<(String, Option<String>, Option<String>, f64)> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut map = std::collections::HashMap::new();
        for (title, romaji, english, score) in rows {
            for variant in [Some(title), romaji, english].into_iter().flatten() {
                map.entry(crate::matching::normalize_title(&variant)).or_insert(score);
            }
        }
        Ok(map)
    }

    /// Normalized-title -> cover_url map from every synced catalog row that has one.
    #[cfg(test)] // superseded in production; kept as a test oracle
    pub fn catalog_covers_by_normalized_title(&self) -> Result<std::collections::HashMap<String, String>> {
        let mut stmt = self.conn.prepare(
            "SELECT title, title_romaji, title_english, cover_url FROM anilist_catalog WHERE cover_url IS NOT NULL AND cover_url != ''",
        )?;
        let rows: Vec<(String, Option<String>, Option<String>, String)> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut map = std::collections::HashMap::new();
        for (title, romaji, english, cover_url) in rows {
            for variant in [Some(title), romaji, english].into_iter().flatten() {
                map.entry(crate::matching::normalize_title(&variant)).or_insert_with(|| cover_url.clone());
            }
        }
        Ok(map)
    }

    /// Query the average score for a specific series row from linked anilist_catalog.
    #[cfg(test)] // superseded in production; kept as a test oracle
    pub fn get_series_score(&self, series_id: i64) -> Result<Option<f64>> {
        let opt: Option<Option<f64>> = self
            .conn
            .query_row(
                "SELECT c.average_score
                 FROM series s
                 LEFT JOIN anilist_catalog c ON c.id = s.anilist_id
                 WHERE s.id = ?1",
                [series_id],
                |r| r.get(0),
            )
            .optional()?;
        Ok(opt.flatten())
    }

    /// ONE query returning all metadata columns used for airing season resolution.
    /// Run under the DB lock; transform via `build_catalog_airing_meta` outside the lock.
    pub fn catalog_airing_meta_rows(&self) -> Result<Vec<CatalogMetaRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT title, title_romaji, title_english, start_date, banner_url, cover_url, average_score FROM anilist_catalog",
        )?;
        let rows = stmt
            .query_map([], |r| {
                Ok(CatalogMetaRow {
                    title: r.get(0)?,
                    title_romaji: r.get(1)?,
                    title_english: r.get(2)?,
                    start_date: r.get(3)?,
                    banner_url: r.get(4)?,
                    cover_url: r.get(5)?,
                    average_score: r.get(6)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Pure transform from raw catalog metadata rows to the normalized-title lookup maps.
    /// Normalizes each title variant once, preserving first-variant-encountered semantics.
    pub fn build_catalog_airing_meta(rows: Vec<CatalogMetaRow>) -> CatalogAiringMeta {
        build_catalog_airing_meta(rows)
    }

    /// Query banner URLs and average scores for multiple series rows in batched queries,
    /// returning COALESCE(s.banner_url, c.banner_url) and c.average_score per series id.
    /// Missing series ids (not present in the `series` table) are absent from the returned map.
    pub fn series_banners_and_scores(
        &self,
        series_ids: &[i64],
    ) -> Result<SeriesBannerScore> {
        let mut map = HashMap::new();
        if series_ids.is_empty() {
            return Ok(map);
        }
        for chunk in series_ids.chunks(500) {
            let placeholders = vec!["?"; chunk.len()].join(",");
            let sql = format!(
                "SELECT s.id, COALESCE(s.banner_url, c.banner_url), c.average_score
                 FROM series s
                 LEFT JOIN anilist_catalog c ON c.id = s.anilist_id
                 WHERE s.id IN ({placeholders})"
            );
            let mut stmt = self.conn.prepare(&sql)?;
            let params = rusqlite::params_from_iter(chunk.iter());
            let rows = stmt
                .query_map(params, |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, Option<String>>(1)?,
                        r.get::<_, Option<f64>>(2)?,
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            for (id, banner, score) in rows {
                map.insert(id, (banner, score));
            }
        }
        Ok(map)
    }

    /// Persist a banner URL for a series row (and its linked anilist_catalog entry, if any).
    pub fn set_series_banner(&self, series_id: i64, banner_url: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE series SET banner_url = ?1 WHERE id = ?2",
            rusqlite::params![banner_url, series_id],
        )?;
        self.conn.execute(
            "UPDATE anilist_catalog SET banner_url = ?1 WHERE id = (SELECT anilist_id FROM series WHERE id = ?2 AND anilist_id IS NOT NULL)",
            rusqlite::params![banner_url, series_id],
        )?;
        Ok(())
    }

    /// Retrieve title and anilist_id for a series.
    pub fn get_series_title_and_anilist_id(&self, series_id: i64) -> Result<(String, Option<i64>)> {
        Ok(self.conn.query_row(
            "SELECT title, anilist_id FROM series WHERE id = ?1",
            [series_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?)
    }

    /// Normalized-title -> `anilist_catalog.id`, mirroring
    /// `catalog_start_dates_by_normalized_title`'s "try every synced title
    /// variant" approach — shared lookup behind `catalog_id_for_series`'s
    /// title-match fallback.
    fn catalog_ids_by_normalized_title(&self) -> Result<std::collections::HashMap<String, i64>> {
        let mut stmt = self.conn.prepare("SELECT id, title, title_romaji, title_english FROM anilist_catalog")?;
        let rows: Vec<(i64, String, Option<String>, Option<String>)> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut map = std::collections::HashMap::new();
        for (id, title, romaji, english) in rows {
            for variant in [Some(title), romaji, english].into_iter().flatten() {
                map.entry(crate::matching::normalize_title(&variant)).or_insert(id);
            }
        }
        Ok(map)
    }

    /// Resolve a `series` row to a matched `anilist_catalog.id`: direct
    /// `anilist_id` FK when it's set AND still points at a synced row (rare
    /// for followed/scraped series — usually only set via the Descubrir
    /// catalog-swipe/link path, see `series::anilist_id`'s doc), otherwise
    /// falling back to the same normalized-title matching
    /// `list_airing_season` already relies on for unlinked series. Used by
    /// `catalog_info_for_series`.
    fn catalog_id_for_series(&self, series_id: i64) -> Result<Option<i64>> {
        let (title, anilist_id): (String, Option<i64>) = self.conn.query_row(
            "SELECT title, anilist_id FROM series WHERE id=?1",
            [series_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        if let Some(id) = anilist_id {
            let linked_row_exists: bool = self
                .conn
                .query_row("SELECT 1 FROM anilist_catalog WHERE id=?1", [id], |_| Ok(()))
                .optional()?
                .is_some();
            if linked_row_exists {
                return Ok(Some(id));
            }
        }
        // The same show on another site (or another row of this one) may be
        // linked even though this row is not — e.g. the fuzzy linker matched
        // the twin, or this site titles it in a way no catalog title matches.
        // `canonical_group_keys` is the library's own identity bridge, so a
        // row that groups with a linked twin shows that twin's AniList data.
        if let Some(key) = self.canonical_group_keys()?.get(&series_id) {
            if let Some(al) = key.strip_prefix("al:").and_then(|n| n.parse::<i64>().ok()) {
                let exists = self
                    .conn
                    .query_row("SELECT 1 FROM anilist_catalog WHERE id=?1", [al], |_| Ok(()))
                    .optional()?
                    .is_some();
                if exists {
                    return Ok(Some(al));
                }
            }
        }
        Ok(self.catalog_ids_by_normalized_title()?.get(&crate::matching::normalize_title(&title)).copied())
    }

    /// Full synced AniList catalog entry (title/cover/genres/format/
    /// episodes/score/studio/etc) matched to a `series` row — the source for
    /// SeriesDetail's info panel now that anime metadata is meant to come
    /// from AniList, not the scraped site (which only still supplies episode
    /// links and the "is it airing" signal). `None` when nothing in the
    /// synced catalog matches this series (not yet synced, or no title/id
    /// match at all).
    pub fn catalog_info_for_series(&self, series_id: i64) -> Result<Option<crate::anilist::CatalogAnime>> {
        let Some(id) = self.catalog_id_for_series(series_id)? else { return Ok(None) };
        let mut stmt = self.conn.prepare(
            "SELECT id, title, title_romaji, title_english, cover_url, banner_url, format, episodes, average_score, popularity, url, status, duration, studio, start_date, next_airing_at, next_episode
             FROM anilist_catalog WHERE id=?1",
        )?;
        let mut anime = stmt.query_row([id], Self::row_to_catalog_anime)?;
        anime.genres = self.list_catalog_genres(id)?;
        anime.tags = self.list_catalog_tags(id)?;
        Ok(Some(anime))
    }

    pub(super) fn row_to_catalog_anime(r: &rusqlite::Row) -> rusqlite::Result<crate::anilist::CatalogAnime> {
        let id: i64 = r.get("id")?;
        Ok(crate::anilist::CatalogAnime {
            id,
            title: r.get("title")?,
            title_romaji: r.get("title_romaji")?,
            title_english: r.get("title_english")?,
            synonyms: Vec::new(),
            cover_url: r.get("cover_url")?,
            banner_url: r.get("banner_url")?,
            format: r.get("format")?,
            episodes: r.get("episodes")?,
            average_score: r.get("average_score")?,
            popularity: r.get("popularity")?,
            url: r.get("url")?,
            status: r.get("status")?,
            duration: r.get("duration")?,
            studio: r.get("studio")?,
            start_date: r.get("start_date")?,
            next_airing_at: r.get("next_airing_at")?,
            next_episode: r.get("next_episode")?,
            genres: Vec::new(), // filled in by callers that need it — see list_catalog
            tags: Vec::new(), // filled in by callers that need it
            relations: Vec::new(),
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
    ///
    /// Test-only: production always goes through `list_catalog_filtered`
    /// (the Catálogo tab always passes a filter, even an empty one); this
    /// unfiltered convenience wrapper is kept for the pagination tests.
    #[cfg(test)]
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

    pub fn list_catalog_tags(&self, anilist_id: i64) -> Result<Vec<crate::anilist::CatalogTag>> {
        let mut stmt = self
            .conn
            .prepare("SELECT tag, rank FROM anilist_catalog_tags WHERE anilist_id=?1 ORDER BY rank DESC, tag")?;
        let tags = stmt
            .query_map([anilist_id], |r| Ok(crate::anilist::CatalogTag { name: r.get(0)?, rank: r.get(1)? }))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(tags)
    }

    pub fn list_catalog_genres_for(&self, ids: &[i64]) -> Result<HashMap<i64, Vec<String>>> {
        let mut map: HashMap<i64, Vec<String>> = HashMap::with_capacity(ids.len());
        if ids.is_empty() {
            return Ok(map);
        }
        for &id in ids {
            map.entry(id).or_default();
        }

        let mut unique_ids: Vec<i64> = ids.to_vec();
        unique_ids.sort_unstable();
        unique_ids.dedup();

        for chunk in unique_ids.chunks(500) {
            let placeholders = vec!["?"; chunk.len()].join(",");
            let sql = format!(
                "SELECT anilist_id, genre FROM anilist_catalog_genres \
                 WHERE anilist_id IN ({placeholders}) \
                 ORDER BY genre"
            );
            let mut stmt = self.conn.prepare(&sql)?;
            let mut rows = stmt.query(rusqlite::params_from_iter(chunk.iter()))?;
            while let Some(row) = rows.next()? {
                let anilist_id: i64 = row.get(0)?;
                let genre: String = row.get(1)?;
                if let Some(list) = map.get_mut(&anilist_id) {
                    list.push(genre);
                }
            }
        }
        Ok(map)
    }

    pub fn list_catalog_tags_for(&self, ids: &[i64]) -> Result<HashMap<i64, Vec<CatalogTag>>> {
        let mut map: HashMap<i64, Vec<CatalogTag>> = HashMap::with_capacity(ids.len());
        if ids.is_empty() {
            return Ok(map);
        }
        for &id in ids {
            map.entry(id).or_default();
        }

        let mut unique_ids: Vec<i64> = ids.to_vec();
        unique_ids.sort_unstable();
        unique_ids.dedup();

        for chunk in unique_ids.chunks(500) {
            let placeholders = vec!["?"; chunk.len()].join(",");
            let sql = format!(
                "SELECT anilist_id, tag, rank FROM anilist_catalog_tags \
                 WHERE anilist_id IN ({placeholders}) \
                 ORDER BY rank DESC, tag"
            );
            let mut stmt = self.conn.prepare(&sql)?;
            let mut rows = stmt.query(rusqlite::params_from_iter(chunk.iter()))?;
            while let Some(row) = rows.next()? {
                let anilist_id: i64 = row.get(0)?;
                let tag: String = row.get(1)?;
                let rank: i64 = row.get(2)?;
                if let Some(list) = map.get_mut(&anilist_id) {
                    list.push(CatalogTag { name: tag, rank });
                }
            }
        }
        Ok(map)
    }

    /// Stored PREQUEL/SEQUEL edges of one entry as (related id, relation type). Only read by tests today:
    /// `releasing_sequel_id` walks the table with its own prepared statements.
    #[cfg(test)]
    pub fn list_catalog_relations(&self, anilist_id: i64) -> Result<Vec<(i64, String)>> {
        let mut stmt = self
            .conn
            .prepare("SELECT related_id, relation_type FROM anilist_catalog_relations WHERE anilist_id=?1 ORDER BY related_id, relation_type")?;
        let relations = stmt
            .query_map([anilist_id], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(relations)
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
}
