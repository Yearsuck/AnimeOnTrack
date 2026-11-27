use super::*;

/// The query behind `Db::stale_status_ids`, shared with its regression test so the
/// plan that is asserted is the plan that runs. It correlates on `series.anilist_id`
/// once per catalog row, so it depends on `idx_series_anilist_id` (see `db.rs`):
/// without the index it took ~220 s on a real library while holding the DB mutex.
pub(super) const STALE_STATUS_IDS_SQL: &str = "SELECT c.id FROM anilist_catalog c
     WHERE EXISTS (SELECT 1 FROM series s WHERE s.anilist_id = c.id AND s.is_airing = 1)
        OR c.status IN ('RELEASING', 'NOT_YET_RELEASED')
        OR (c.status = 'FINISHED' AND c.start_date IS NOT NULL AND c.start_date >= ?1)
     ORDER BY (c.next_airing_at IS NOT NULL AND c.next_airing_at <= ?2) DESC,
              EXISTS (SELECT 1 FROM series s WHERE s.anilist_id = c.id) DESC,
              COALESCE(c.popularity, 0) DESC,
              c.id
     LIMIT 500";

impl Db {
    /// Insert or refresh one AniList catalog entry, keyed by AniList's own
    /// numeric id (reused as our primary key — no reason to mint a separate
    /// one). `sort_order` is the position in the popularity-sorted sync
    /// sequence, so local pagination preserves the same ordering AniList's
    /// own `POPULARITY_DESC` sort gave it.
    ///
    /// Every write stamps `metadata_version = CATALOG_METADATA_VERSION`: a
    /// `CatalogAnime` always carries the current full field set (the GraphQL
    /// query in `anilist.rs` requests all of them), so reaching this function
    /// at all means the row is up to date with today's schema — including when
    /// AniList genuinely has no `studio`/`duration`/`start_date` for it, which
    /// is real data and no longer indistinguishable from "never fetched". See
    /// `stale_catalog_ids`.
    pub fn upsert_catalog_anime(
        &self,
        anime: &crate::anilist::CatalogAnime,
        sort_order: i64,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO anilist_catalog(id, title, title_romaji, title_english, cover_url, banner_url, format, episodes, average_score, popularity, url, sort_order, status, duration, studio, start_date, metadata_version, next_airing_at, next_episode)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19)
             ON CONFLICT(id) DO UPDATE SET
                title=excluded.title, title_romaji=excluded.title_romaji, title_english=excluded.title_english,
                cover_url=excluded.cover_url,
                banner_url=COALESCE(excluded.banner_url, anilist_catalog.banner_url),
                format=excluded.format,
                episodes=excluded.episodes, average_score=excluded.average_score,
                popularity=excluded.popularity, url=excluded.url, sort_order=excluded.sort_order,
                status=excluded.status, duration=excluded.duration, studio=excluded.studio,
                start_date=excluded.start_date, metadata_version=excluded.metadata_version,
                next_airing_at=excluded.next_airing_at, next_episode=excluded.next_episode",
            // rusqlite only implements `Params` for tuples up to 16 elements.
            rusqlite::params![
                anime.id, &anime.title, &anime.title_romaji, &anime.title_english, &anime.cover_url,
                &anime.banner_url, &anime.format,
                anime.episodes, anime.average_score, anime.popularity, &anime.url, sort_order, &anime.status,
                anime.duration, &anime.studio, anime.start_date, CATALOG_METADATA_VERSION,
                anime.next_airing_at, anime.next_episode,
            ],
        )?;
        self.conn.execute("DELETE FROM anilist_catalog_genres WHERE anilist_id=?1", [anime.id])?;
        for genre in &anime.genres {
            self.conn.execute(
                "INSERT OR IGNORE INTO anilist_catalog_genres(anilist_id, genre) VALUES(?1, ?2)",
                (anime.id, genre),
            )?;
        }
        self.conn.execute("DELETE FROM anilist_catalog_synonyms WHERE anilist_id=?1", [anime.id])?;
        for synonym in &anime.synonyms {
            self.conn.execute(
                "INSERT OR IGNORE INTO anilist_catalog_synonyms(anilist_id, synonym) VALUES(?1, ?2)",
                (anime.id, synonym),
            )?;
        }
        self.conn.execute("DELETE FROM anilist_catalog_tags WHERE anilist_id=?1", [anime.id])?;
        for tag in &anime.tags {
            self.conn.execute(
                "INSERT OR IGNORE INTO anilist_catalog_tags(anilist_id, tag, rank) VALUES(?1, ?2, ?3)",
                (anime.id, &tag.name, tag.rank),
            )?;
        }
        self.conn.execute("DELETE FROM anilist_catalog_relations WHERE anilist_id=?1", [anime.id])?;
        for relation in &anime.relations {
            self.conn.execute(
                "INSERT OR IGNORE INTO anilist_catalog_relations(anilist_id, related_id, relation_type) VALUES(?1, ?2, ?3)",
                (anime.id, relation.related_id, &relation.relation_type),
            )?;
        }
        Ok(())
    }

    /// Same writes as `upsert_catalog_anime`, batched under one transaction —
    /// for the full/incremental catalog sync and the metadata backfill, which
    /// otherwise call it once per row (up to ~22,000 times for a full sync),
    /// each paying for its own implicit transaction/fsync. Call once per
    /// network-response page/batch rather than per row. `unchecked_transaction`
    /// borrows `&self.conn` immutably (rusqlite's usual interior-mutability
    /// pattern), so reusing `upsert_catalog_anime` — itself `&self` — for each
    /// row here is fine: every one of its statements runs inside this same
    /// open transaction rather than its own implicit one.
    pub fn upsert_catalog_anime_batch(
        &self,
        items: &[(&crate::anilist::CatalogAnime, i64)],
    ) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        for (anime, sort_order) in items {
            self.upsert_catalog_anime(anime, *sort_order)?;
        }
        tx.commit()?;
        Ok(())
    }

    /// AniList ids of catalog rows that predate the current metadata field
    /// set — i.e. whose `metadata_version` is behind `CATALOG_METADATA_VERSION`.
    ///
    /// This used to be inferred from `title_romaji IS NULL OR = ''`, which
    /// only ever caught rows predating the *first* extra column. `title_romaji`
    /// shipped before `status`/`duration`/`studio`/`start_date` did, so every
    /// row synced in that window had a romaji title and NULL everything else
    /// and was permanently invisible to the backfill — and the same trap was
    /// waiting for the next column added. Column-NULL-ness also can't tell a
    /// never-fetched field from one AniList genuinely has no value for (plenty
    /// of titles have no credited studio or no fully-specified premiere date);
    /// an explicit version stamp can. See `CATALOG_METADATA_VERSION` and the
    /// `metadata_version` migration in `db::init_schema`.
    ///
    /// Rows are ordered so the ones a user can actually feel come back first —
    /// those linked to one of their own series, then the most popular —
    /// because this backfill is long enough to be interrupted and resumed, and
    /// an interrupted run should already have fixed the rows that matter.
    pub fn stale_catalog_ids(&self) -> Result<Vec<i64>> {
        let mut stmt = self.conn.prepare(
            "SELECT c.id FROM anilist_catalog c
             WHERE c.metadata_version < ?1
             ORDER BY EXISTS (SELECT 1 FROM series s WHERE s.anilist_id = c.id) DESC,
                      COALESCE(c.popularity, 0) DESC,
                      c.id",
        )?;
        let ids = stmt
            .query_map([CATALOG_METADATA_VERSION], |r| r.get::<_, i64>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(ids)
    }

    /// AniList ids of catalog rows whose `status` needs refreshing:
    /// (a) linked to a series with `is_airing = 1`, or
    /// (b) status in `('RELEASING', 'NOT_YET_RELEASED')`, or
    /// (c) status = `'FINISHED'` but `start_date` within the last 120 days.
    ///
    /// Capped at 500 rows and ordered: rows that have already aired (next_airing_at <= now)
    /// first, then linked-first, then by popularity, so shows needing status rollover or
    /// that the user actually follows/watches take priority.
    pub fn stale_status_ids(&self) -> Result<Vec<i64>> {
        let now = chrono::Utc::now().timestamp();
        let cutoff = (chrono::Utc::now() - chrono::Duration::days(120)).timestamp();
        let mut stmt = self.conn.prepare(STALE_STATUS_IDS_SQL)?;
        let ids = stmt
            .query_map(rusqlite::params![cutoff, now], |r| r.get::<_, i64>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(ids)
    }

    /// Existing `sort_order` for a catalog row, so a metadata backfill can
    /// hand it straight back to `upsert_catalog_anime` instead of overwriting
    /// the row's place in the synced ordering with a fresh counter. Falls back
    /// to 0 for an id that isn't stored (which a backfill shouldn't hit, but
    /// this must not be the thing that fails the run).
    pub fn catalog_sort_order(&self, id: i64) -> Result<i64> {
        Ok(self
            .conn
            .query_row("SELECT sort_order FROM anilist_catalog WHERE id=?1", [id], |r| r.get(0))
            .optional()?
            .unwrap_or(0))
    }

    /// Replace the stored ani.zip episode map for `anilist_id` with `entries`.
    /// Runs in a single transaction: deletes existing mapped episodes, inserts
    /// the new entries, and upserts the `anilist_episode_map_fetched` tracking row.
    pub fn replace_episode_map(
        &self,
        anilist_id: i64,
        entries: &[crate::anizip::MapEntry],
        now: i64,
    ) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        self.conn.execute(
            "DELETE FROM anilist_episode_map WHERE anilist_id = ?1",
            [anilist_id],
        )?;
        for entry in entries {
            self.conn.execute(
                "INSERT INTO anilist_episode_map (anilist_id, episode, season, absolute, air_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params![anilist_id, entry.episode, entry.season, entry.absolute, entry.air_at],
            )?;
        }
        self.conn.execute(
            "INSERT INTO anilist_episode_map_fetched (anilist_id, fetched_at, entries)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(anilist_id) DO UPDATE SET
                 fetched_at = excluded.fetched_at,
                 entries = excluded.entries",
            rusqlite::params![anilist_id, now, entries.len() as i64],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// AniList ids of series that are followed OR airing, have a linked AniList id,
    /// and either have no fetched episode map or a stale one (older than 3 days for RELEASING/HIATUS,
    /// older than 30 days otherwise). Distinct, ordered by airing/followed first, then oldest fetched,
    /// capped at `limit`.
    pub fn episode_map_stale_ids(&self, now: i64, limit: usize) -> Result<Vec<i64>> {
        let three_days_ago = now - 3 * 86400;
        let thirty_days_ago = now - 30 * 86400;
        let mut stmt = self.conn.prepare(
            "SELECT s.anilist_id
             FROM series s
             LEFT JOIN anilist_episode_map_fetched f ON f.anilist_id = s.anilist_id
             LEFT JOIN anilist_catalog c ON c.id = s.anilist_id
             WHERE (s.followed = 1 OR s.is_airing = 1)
               AND s.anilist_id IS NOT NULL
               AND (
                   f.fetched_at IS NULL
                   OR (
                       CASE
                           WHEN c.status IN ('RELEASING', 'HIATUS') THEN f.fetched_at < ?1
                           ELSE f.fetched_at < ?2
                       END
                   )
               )
             GROUP BY s.anilist_id
             ORDER BY
                 MAX(CASE WHEN s.is_airing = 1 AND s.followed = 1 THEN 2 WHEN s.is_airing = 1 OR s.followed = 1 THEN 1 ELSE 0 END) DESC,
                 COALESCE(MIN(f.fetched_at), 0) ASC,
                 s.anilist_id ASC
             LIMIT ?3",
        )?;
        let rows = stmt
            .query_map(rusqlite::params![three_days_ago, thirty_days_ago, limit as i64], |r| r.get::<_, i64>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// `absolute - episode` of the lowest regular episode when its `absolute` is not NULL.
    /// None when there is no map, no entries, or absolute is NULL.
    #[allow(dead_code)] // A later phase uses this helper.
    pub fn episode_offset(&self, anilist_id: i64) -> Result<Option<i64>> {
        let row: Option<(i64, Option<i64>)> = self
            .conn
            .query_row(
                "SELECT episode, absolute
                 FROM anilist_episode_map
                 WHERE anilist_id = ?1
                 ORDER BY episode ASC
                 LIMIT 1",
                [anilist_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        Ok(row.and_then(|(ep, abs_opt)| abs_opt.map(|abs| abs - ep)))
    }

    /// The per-entry episode with that absolute number, if mapped.
    #[allow(dead_code)] // A later phase uses this helper.
    pub fn episode_number_for_absolute(&self, anilist_id: i64, absolute: i64) -> Result<Option<i64>> {
        let row: Option<i64> = self
            .conn
            .query_row(
                "SELECT episode
                 FROM anilist_episode_map
                 WHERE anilist_id = ?1 AND absolute = ?2
                 LIMIT 1",
                rusqlite::params![anilist_id, absolute],
                |r| r.get(0),
            )
            .optional()?;
        Ok(row)
    }

    /// Upcoming episode and its air timestamp (in seconds) for `anilist_id` from
    /// `anilist_catalog` when `next_airing_at > now` and both columns are non-NULL.
    /// Returns None when either column is NULL, the air date is not strictly in the
    /// future, or no catalog row exists.
    pub fn catalog_next_airing(&self, anilist_id: i64, now: i64) -> Result<Option<(i64, i64)>> {
        let row: Option<(i64, i64)> = self
            .conn
            .query_row(
                "SELECT next_episode, next_airing_at
                 FROM anilist_catalog
                 WHERE id = ?1
                   AND next_airing_at IS NOT NULL
                   AND next_episode IS NOT NULL
                   AND next_airing_at > ?2",
                rusqlite::params![anilist_id, now],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        Ok(row)
    }

    /// Upcoming episode and its air timestamp (in seconds) for `anilist_id`.
    ///
    /// Returns `catalog_next_airing` when that has a value. Otherwise, if the catalog row
    /// has status `RELEASING`, `next_airing_at <= now`, and expired less than 7 days ago,
    /// rolls forward by weekly intervals (matching the `list_airing` date rollover logic).
    /// Returns None when not releasing, air date is too old, columns are NULL, or no row exists.
    pub fn catalog_next_airing_rolled(&self, anilist_id: i64, now: i64) -> Result<Option<(i64, i64, bool)>> {
        if let Some((ep, air)) = self.catalog_next_airing(anilist_id, now)? {
            return Ok(Some((ep, air, false)));
        }

        const SEVEN_DAYS_SECS: i64 = 7 * 86400;
        let row: Option<(i64, i64)> = self
            .conn
            .query_row(
                "SELECT next_episode, next_airing_at
                 FROM anilist_catalog
                 WHERE id = ?1
                   AND status = 'RELEASING'
                   AND next_airing_at IS NOT NULL
                   AND next_episode IS NOT NULL
                   AND next_airing_at <= ?2
                   AND ?2 - next_airing_at < ?3",
                rusqlite::params![anilist_id, now, SEVEN_DAYS_SECS],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;

        let Some((next_episode, next_airing_at)) = row else {
            return Ok(None);
        };

        let mut rolled = next_airing_at;
        let mut k = 0i64;
        while rolled <= now {
            rolled += SEVEN_DAYS_SECS;
            k += 1;
        }

        Ok(Some((next_episode + k, rolled, true)))
    }

    /// Smallest future `air_at` (strictly > `now`) and its episode for `anilist_id`.
    /// Ties on `air_at` broken by lowest episode. Ignores entries with NULL `air_at`.
    /// Returns None when no map exists or nothing upcoming.
    pub fn next_airing_episode(&self, anilist_id: i64, now: i64) -> Result<Option<(i64, i64)>> {
        let row: Option<(i64, i64)> = self
            .conn
            .query_row(
                "SELECT episode, air_at
                 FROM anilist_episode_map
                 WHERE anilist_id = ?1 AND air_at > ?2
                 ORDER BY air_at ASC, episode ASC
                 LIMIT 1",
                rusqlite::params![anilist_id, now],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        Ok(row)
    }

    /// Every catalog row ordered by `sort_order, id` with full fidelity (genres, synonyms,
    /// tags sorted by rank asc then name, relations sorted by related_id, relation_type).
    ///
    /// Reads each child table in a single bulk query into a map, avoiding N+1 queries.
    #[allow(dead_code)] // Called by catalog_snapshot::export_to_bytes and tests.
    pub fn catalog_all_for_snapshot(&self) -> Result<Vec<crate::anilist::CatalogAnime>> {
        let mut genres_map: std::collections::HashMap<i64, Vec<String>> = std::collections::HashMap::new();
        let mut stmt = self
            .conn
            .prepare("SELECT anilist_id, genre FROM anilist_catalog_genres ORDER BY anilist_id, rowid")?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            let anilist_id: i64 = row.get(0)?;
            let genre: String = row.get(1)?;
            genres_map.entry(anilist_id).or_default().push(genre);
        }

        let mut synonyms_map: std::collections::HashMap<i64, Vec<String>> = std::collections::HashMap::new();
        let mut stmt = self
            .conn
            .prepare("SELECT anilist_id, synonym FROM anilist_catalog_synonyms ORDER BY anilist_id, rowid")?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            let anilist_id: i64 = row.get(0)?;
            let synonym: String = row.get(1)?;
            synonyms_map.entry(anilist_id).or_default().push(synonym);
        }

        let mut tags_map: std::collections::HashMap<i64, Vec<crate::anilist::CatalogTag>> =
            std::collections::HashMap::new();
        let mut stmt = self
            .conn
            .prepare("SELECT anilist_id, tag, rank FROM anilist_catalog_tags ORDER BY anilist_id, rank ASC, tag ASC")?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            let anilist_id: i64 = row.get(0)?;
            let name: String = row.get(1)?;
            let rank: i64 = row.get(2)?;
            tags_map
                .entry(anilist_id)
                .or_default()
                .push(crate::anilist::CatalogTag { name, rank });
        }

        let mut relations_map: std::collections::HashMap<i64, Vec<crate::anilist::CatalogRelation>> =
            std::collections::HashMap::new();
        let mut stmt = self.conn.prepare(
            "SELECT anilist_id, related_id, relation_type FROM anilist_catalog_relations ORDER BY anilist_id, related_id ASC, relation_type ASC",
        )?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            let anilist_id: i64 = row.get(0)?;
            let related_id: i64 = row.get(1)?;
            let relation_type: String = row.get(2)?;
            relations_map
                .entry(anilist_id)
                .or_default()
                .push(crate::anilist::CatalogRelation { related_id, relation_type });
        }

        let mut stmt = self.conn.prepare(
            "SELECT id, title, title_romaji, title_english, cover_url, banner_url, format,
                    episodes, average_score, popularity, url, status, duration, studio,
                    start_date, next_airing_at, next_episode
             FROM anilist_catalog
             ORDER BY sort_order, id",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<String>>(2)?,
                r.get::<_, Option<String>>(3)?,
                r.get::<_, Option<String>>(4)?,
                r.get::<_, Option<String>>(5)?,
                r.get::<_, Option<String>>(6)?,
                r.get::<_, Option<i64>>(7)?,
                r.get::<_, Option<i64>>(8)?,
                r.get::<_, Option<i64>>(9)?,
                r.get::<_, String>(10)?,
                r.get::<_, Option<String>>(11)?,
                r.get::<_, Option<i64>>(12)?,
                r.get::<_, Option<String>>(13)?,
                r.get::<_, Option<i64>>(14)?,
                r.get::<_, Option<i64>>(15)?,
                r.get::<_, Option<i64>>(16)?,
            ))
        })?;

        let mut items = Vec::new();
        for row in rows {
            let (
                id,
                title,
                title_romaji,
                title_english,
                cover_url,
                banner_url,
                format,
                episodes,
                average_score,
                popularity,
                url,
                status,
                duration,
                studio,
                start_date,
                next_airing_at,
                next_episode,
            ) = row?;
            let genres = genres_map.remove(&id).unwrap_or_default();
            let synonyms = synonyms_map.remove(&id).unwrap_or_default();
            let mut tags = tags_map.remove(&id).unwrap_or_default();
            tags.sort_by(|a, b| a.rank.cmp(&b.rank).then_with(|| a.name.cmp(&b.name)));
            let mut relations = relations_map.remove(&id).unwrap_or_default();
            relations.sort_by(|a, b| {
                a.related_id
                    .cmp(&b.related_id)
                    .then_with(|| a.relation_type.cmp(&b.relation_type))
            });

            items.push(crate::anilist::CatalogAnime {
                id,
                title,
                title_romaji,
                title_english,
                synonyms,
                cover_url,
                banner_url,
                format,
                genres,
                episodes,
                average_score,
                popularity,
                url,
                status,
                duration,
                studio,
                start_date,
                next_airing_at,
                next_episode,
                tags,
                relations,
            });
        }
        Ok(items)
    }

    /// Imports snapshot items that are not already present in `anilist_catalog`, preserving
    /// local rows without overwriting. Inserts in chunks of 500 using `upsert_catalog_anime_batch`,
    /// assigning `sort_order = base_sort_order + index in items`, so a caller importing in chunks keeps one global order.
    #[allow(dead_code)] // Called by catalog_snapshot::import_from_bytes and tests.
    pub fn import_catalog_snapshot(&self, items: &[crate::anilist::CatalogAnime], base_sort_order: i64) -> Result<usize> {
        let mut existing_ids: std::collections::HashSet<i64> = std::collections::HashSet::new();
        let mut stmt = self.conn.prepare("SELECT id FROM anilist_catalog")?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            existing_ids.insert(row.get(0)?);
        }

        let mut to_insert: Vec<(&crate::anilist::CatalogAnime, i64)> = Vec::new();
        for (idx, anime) in items.iter().enumerate() {
            if existing_ids.insert(anime.id) {
                to_insert.push((anime, base_sort_order + idx as i64));
            }
        }

        let inserted_count = to_insert.len();
        for chunk in to_insert.chunks(500) {
            self.upsert_catalog_anime_batch(chunk)?;
        }
        Ok(inserted_count)
    }
}
