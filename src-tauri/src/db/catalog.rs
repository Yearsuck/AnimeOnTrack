use super::*;
use rusqlite::types::Value;
use std::collections::HashMap;

/// (franchise key, season) -> linked series (id, title, anilist_id).
type SiblingIndex = std::collections::HashMap<(String, i64), Vec<(i64, String, i64)>>;

/// Which generation of AniList fields `upsert_catalog_anime` writes.
///
/// Bump this by one whenever a new AniList-sourced column is added to
/// `anilist_catalog` (and requested in `anilist.rs`'s GraphQL query): every
/// row still stamped with a lower version is then automatically picked up by
/// `stale_catalog_ids` and re-fetched by the metadata backfill. This replaces
/// the old "is `title_romaji` NULL?" heuristic, which could only ever detect
/// rows predating the *first* extra column and misread a legitimately-absent
/// `studio`/`start_date` as staleness. See the `metadata_version` migration in
/// `db::init_schema`.
///
/// Version 1 = id, title, title_romaji, title_english, cover_url, banner_url,
/// format, episodes, average_score, popularity, url, status, duration, studio,
/// start_date, genres.
/// Version 2 = synonyms.
/// Version 3 = next airing episode and tags.
/// Version 4 = relations.
pub const CATALOG_METADATA_VERSION: i64 = 4;

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
    /// Inclusive ceiling on `average_score` (0-100 scale).
    pub max_score: Option<i64>,
    /// Inclusive floor on `start_date` (Unix timestamp, seconds).
    pub min_start_date: Option<i64>,
    /// Inclusive ceiling on `start_date` (Unix timestamp, seconds).
    pub max_start_date: Option<i64>,
    /// Exact match against AniList status string (RELEASING/FINISHED/NOT_YET_RELEASED/CANCELLED/HIATUS).
    pub status: Option<String>,
    /// Ordering: popularity (default), score_desc, score_asc, newest, oldest, title.
    pub sort: Option<String>,
    /// Episode-count bucket: "1" | "2-12" | "13-26" | "27+" | "unknown"
    /// (`unknown` = `episodes IS NULL`). Unrecognized values are ignored.
    pub episodes: Option<String>,
    /// Exact match against `anilist_catalog.studio`.
    pub studio: Option<String>,
    /// When true, exclude anime the user has already watched (watched_externally=1 or seen episodes).
    #[serde(default)]
    pub hide_watched: bool,
}

/// Per series id: (own-or-linked banner_url, linked average_score).
pub type SeriesBannerScore = HashMap<i64, (Option<String>, Option<f64>)>;

/// Raw row from `anilist_catalog` containing metadata fields for airing season resolution.
#[derive(Debug, Clone, PartialEq)]
pub struct CatalogMetaRow {
    pub title: String,
    pub title_romaji: Option<String>,
    pub title_english: Option<String>,
    pub start_date: Option<i64>,
    pub banner_url: Option<String>,
    pub cover_url: Option<String>,
    pub average_score: Option<f64>,
}

/// Normalized-title indexed maps for fast airing season metadata resolution.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CatalogAiringMeta {
    pub start_dates: HashMap<String, i64>,
    pub banners: HashMap<String, String>,
    pub covers: HashMap<String, String>,
    pub scores: HashMap<String, f64>,
}

/// Pure transform from raw catalog metadata rows to the normalized-title lookup maps.
/// Normalizes each title variant once and inserts into each map with entry().or_insert
/// only when that field passes the same filter the old functions used:
/// - start_date not NULL
/// - banner_url not NULL and not empty
/// - cover_url not NULL and not empty
/// - average_score not NULL
///
/// First variant encountered wins, per map.
pub fn build_catalog_airing_meta(rows: Vec<CatalogMetaRow>) -> CatalogAiringMeta {
    let mut meta = CatalogAiringMeta::default();

    for row in rows {
        let mut variants: Vec<String> = Vec::with_capacity(3);
        for raw in [Some(&row.title), row.title_romaji.as_ref(), row.title_english.as_ref()]
            .into_iter()
            .flatten()
        {
            let norm = crate::matching::normalize_title(raw);
            if !variants.iter().any(|v| v == &norm) {
                variants.push(norm);
            }
        }

        if let Some(start_date) = row.start_date {
            for v in &variants {
                meta.start_dates.entry(v.clone()).or_insert(start_date);
            }
        }
        if let Some(banner_url) = row.banner_url.filter(|s| !s.is_empty()) {
            for v in &variants {
                meta.banners.entry(v.clone()).or_insert_with(|| banner_url.clone());
            }
        }
        if let Some(cover_url) = row.cover_url.filter(|s| !s.is_empty()) {
            for v in &variants {
                meta.covers.entry(v.clone()).or_insert_with(|| cover_url.clone());
            }
        }
        if let Some(score) = row.average_score {
            for v in &variants {
                meta.scores.entry(v.clone()).or_insert(score);
            }
        }
    }

    meta
}

/// The query behind `Db::stale_status_ids`, shared with its regression test so the
/// plan that is asserted is the plan that runs. It correlates on `series.anilist_id`
/// once per catalog row, so it depends on `idx_series_anilist_id` (see `db.rs`):
/// without the index it took ~220 s on a real library while holding the DB mutex.
const STALE_STATUS_IDS_SQL: &str = "SELECT c.id FROM anilist_catalog c
     WHERE EXISTS (SELECT 1 FROM series s WHERE s.anilist_id = c.id AND s.is_airing = 1)
        OR c.status IN ('RELEASING', 'NOT_YET_RELEASED')
        OR (c.status = 'FINISHED' AND c.start_date IS NOT NULL AND c.start_date >= ?1)
     ORDER BY EXISTS (SELECT 1 FROM series s WHERE s.anilist_id = c.id) DESC,
              COALESCE(c.popularity, 0) DESC,
              c.id
     LIMIT 500";

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

    /// Every catalog row's title variants, for building a
    /// `matching::CatalogIndex`. All three stored spellings are returned
    /// because the scraped site lists some shows under their romaji title and
    /// others under the English one, and the display `title` column is only
    /// ever one of the two.
    pub fn catalog_titles_for_index(&self) -> Result<Vec<crate::matching::CatalogTitleRow>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, title, title_romaji, title_english, popularity FROM anilist_catalog")?;
        let mut rows = stmt
            .query_map([], |r| {
                let id: i64 = r.get(0)?;
                let title: String = r.get(1)?;
                let romaji: Option<String> = r.get(2)?;
                let english: Option<String> = r.get(3)?;
                let popularity: Option<i64> = r.get(4)?;
                let mut titles = vec![title];
                titles.extend(romaji);
                titles.extend(english);
                Ok(crate::matching::CatalogTitleRow {
                    id,
                    titles,
                    synonyms: Vec::new(),
                    popularity: popularity.unwrap_or(0),
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        let mut syn_stmt = self.conn.prepare("SELECT anilist_id, synonym FROM anilist_catalog_synonyms")?;
        let mut syn_map: std::collections::HashMap<i64, Vec<String>> = std::collections::HashMap::new();
        let syn_rows = syn_stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?;
        for res in syn_rows {
            let (id, syn) = res?;
            syn_map.entry(id).or_default().push(syn);
        }

        for row in &mut rows {
            if let Some(syns) = syn_map.remove(&row.id) {
                for syn in syns {
                    let trimmed = syn.trim();
                    // Under 4 chars is too ambiguous to match on.
                    if trimmed.chars().count() >= 4
                        && !row.titles.iter().chain(&row.synonyms).any(|t| t.eq_ignore_ascii_case(trimmed))
                    {
                        row.synonyms.push(trimmed.to_string());
                    }
                }
            }
        }
        Ok(rows)
    }

    /// Unlinked series worth resolving to a catalog row: the ones the user has
    /// engaged with, plus every airing show. Scraped-but-untouched rows (the
    /// bulk of the table) are skipped — linking them buys nothing and multiplies
    /// the chance of a wrong automatic link.
    ///
    /// **Across all sites** (no source filter): the canonical airing/library
    /// unions dedup by `anilist_id`, so a show must be linked on *every* site
    /// that has it, not just the active one — otherwise the same show appears
    /// twice (linked on one site, title-only on another).
    pub fn series_needing_catalog_link(&self) -> Result<Vec<(i64, String)>> {
        let mut stmt = self.conn.prepare(
            "SELECT s.id, s.title FROM series s
             WHERE s.anilist_id IS NULL
               AND (s.followed=1 OR s.watched_externally=1 OR s.is_airing=1
                    OR EXISTS (SELECT 1 FROM episodes e WHERE e.series_id=s.id AND e.seen=1))
             ORDER BY s.id",
        )?;
        let rows = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }
}

/// One title the local catalog could not link, with every series row that
/// carries it (same normalized title across sites), for an AniList search.
#[derive(Debug, Clone)]
pub struct SearchTarget {
    pub norm_title: String,
    pub title: String,
    pub series_ids: Vec<i64>,
}

impl Db {
    pub fn unlinked_for_search(&self, limit: usize, now: i64) -> Result<Vec<SearchTarget>> {
        // Misses were decided by older matching rules: when the rules improve,
        // every title gets another chance instead of waiting out the 14 days.
        const SEARCH_RULES_VERSION: &str = "2";
        if self.get_setting("anilist_search_rules")?.as_deref() != Some(SEARCH_RULES_VERSION) {
            self.conn.execute("DELETE FROM anilist_search_misses", [])?;
            self.set_setting("anilist_search_rules", SEARCH_RULES_VERSION)?;
        }
        let mut by_norm: std::collections::HashMap<String, SearchTarget> = std::collections::HashMap::new();
        let mut order: Vec<String> = Vec::new();

        let mut stmt = self.conn.prepare(
            "SELECT s.id, s.title, (s.is_airing = 1 OR s.followed = 1) as priority
             FROM series s
             WHERE s.anilist_id IS NULL
               AND (s.followed=1 OR s.watched_externally=1 OR s.is_airing=1
                    OR EXISTS (SELECT 1 FROM episodes e WHERE e.series_id=s.id AND e.seen=1))
             ORDER BY priority DESC, s.id"
        )?;

        let rows = stmt.query_map([], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
        })?.collect::<rusqlite::Result<Vec<_>>>()?;

        let mut check_miss = self.conn.prepare("SELECT tried_at FROM anilist_search_misses WHERE norm_title=?1")?;

        for (id, title) in rows {
            let norm = crate::matching::normalize_title(&title);
            if norm.is_empty() { continue; }

            if !by_norm.contains_key(&norm) {
                let tried_at: Option<i64> = check_miss.query_row([&norm], |r| r.get(0)).optional()?;
                if let Some(t) = tried_at {
                    if now - t < 14 * 24 * 3600 {
                        continue;
                    }
                }
                by_norm.insert(norm.clone(), SearchTarget {
                    norm_title: norm.clone(),
                    title: title.clone(),
                    series_ids: Vec::new(),
                });
                order.push(norm.clone());
            }
            if let Some(entry) = by_norm.get_mut(&norm) {
                entry.series_ids.push(id);
            }
        }

        let mut result = Vec::new();
        for norm in order {
            if result.len() >= limit { break; }
            if let Some(target) = by_norm.remove(&norm) {
                result.push(target);
            }
        }

        Ok(result)
    }

    pub fn record_search_miss(&self, norm_title: &str, now: i64) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO anilist_search_misses(norm_title, tried_at) VALUES(?1, ?2)",
            rusqlite::params![norm_title, now],
        )?;
        Ok(())
    }

    /// Store the AniList entry found by a search (so the series has a catalog
    /// row to read metadata from) and link every row of the target to it.
    pub fn link_search_result(&self, series_ids: &[i64], anime: &crate::anilist::CatalogAnime) -> Result<()> {
        let sort_order = self.catalog_sort_order(anime.id)?;
        self.upsert_catalog_anime(anime, sort_order)?;
        for &id in series_ids {
            self.set_anilist_id(id, anime.id)?;
        }
        Ok(())
    }

    /// Linked series grouped by (franchise key, season), built once per run so
    /// the sibling step is a hash lookup instead of re-normalising every linked
    /// title for every candidate (that O(n*m) loop cost seconds at real size
    /// while the DB mutex was held).
    fn sibling_index(linked: &[(i64, String, i64)]) -> SiblingIndex {
        let mut map: SiblingIndex = std::collections::HashMap::new();
        for (id, title, anilist_id) in linked {
            let norm = crate::matching::normalize_title(title);
            // Ambiguous seasons ("The Final Season") must not pool with season 1.
            let Some(season) = crate::matching::extract_season_number(&norm) else { continue };
            map.entry((crate::matching::franchise_dedup_key(title), season as i64))
                .or_default()
                .push((*id, title.clone(), *anilist_id));
        }
        map
    }

    /// A show long-running enough that the sites split it into arc rows
    /// ("One Piece: Arco de Elbaph"). Only those may inherit their parent's
    /// link from the colon-stripped base title; for a short show the colon
    /// tail is a season ("Kami no Tou: Ouji no Kikan" is Tower of God season
    /// 2, not season 1). AniList leaves `episodes` empty while a show airs.
    fn is_long_runner(&self, anilist_id: i64) -> Result<bool> {
        let episodes: Option<i64> = self
            .conn
            .query_row("SELECT episodes FROM anilist_catalog WHERE id=?1", [anilist_id], |r| r.get(0))
            .optional()?
            .flatten();
        Ok(episodes.is_none_or(|n| n >= 60))
    }

    fn resolve_catalog_link(
        &self,

        title: &str,
        ignore_series_id: i64,
        index: &crate::matching::CatalogIndex,
        catalog_titles: &std::collections::HashMap<i64, Vec<String>>,
        siblings: &SiblingIndex,
    ) -> Result<Option<i64>> {
        // A live-action adaptation is a different work: it may only link to an
        // entry that is itself live action, never to the anime (fuzzy matching
        // would otherwise pick the anime again on every run).
        let live = crate::matching::is_live_action(title);
        let is_consistent = |id: i64| -> bool {
            if let Some(titles) = catalog_titles.get(&id) {
                if live && !titles.iter().any(|t| crate::matching::is_live_action(t)) {
                    return false;
                }
                let refs: Vec<&str> = titles.iter().map(String::as_str).collect();
                crate::matching::season_consistent(title, &refs)
            } else {
                false
            }
        };

        if let Some(id) = index.lookup(&[title]) {
            if is_consistent(id) { return Ok(Some(id)); }
        }
        if let Some(id) = index.fuzzy_lookup(&[title]) {
            if is_consistent(id) { return Ok(Some(id)); }
        }

        let norm = crate::matching::normalize_title(title);
        let s = crate::matching::extract_season_number(&norm);
        // Only a clearly season-1 title may fall back to its base ("X (2011)",
        // "X Part 1"); an ambiguous one ("X The Final Season") must stay unlinked
        // rather than join season 1.
        if s == Some(1) && !live {
            let base = crate::db::stats::franchise_base_title(title);
            if base != title {
                let colon = title.contains(':');
                for id in [index.lookup(&[&base]), index.fuzzy_lookup(&[&base])].into_iter().flatten() {
                    if is_consistent(id) && (!colon || self.is_long_runner(id)?) {
                        return Ok(Some(id));
                    }
                }
            }
        }

        // Sites spell season 2 as "X Temporada 2" while AniList often lists
        // the same season as "X 2" (a synonym or the romaji): try that spelling.
        if let Some(n) = s.filter(|&n| n >= 2 && !live) {
            let tokens: Vec<&str> = title.split_whitespace().collect();
            let stem = crate::matching::strip_season_markers(tokens, crate::matching::normalize_title).join(" ");
            if !stem.is_empty() {
                let bare = format!("{stem} {n}");
                for id in [index.lookup(&[&bare]), index.fuzzy_lookup(&[&bare])].into_iter().flatten() {
                    if is_consistent(id) {
                        return Ok(Some(id));
                    }
                }
            }
        }

        let Some(season) = s.filter(|_| !live) else {
            return Ok(None);
        };
        let key = (crate::matching::franchise_dedup_key(title), season as i64);
        let mut candidate_ids = std::collections::HashSet::new();
        for (sib_id, sib_title, sib_anilist_id) in siblings.get(&key).into_iter().flatten() {
            if *sib_id == ignore_series_id { continue; }
            if let Some(titles) = catalog_titles.get(sib_anilist_id) {
                let refs: Vec<&str> = titles.iter().map(String::as_str).collect();
                if crate::matching::season_consistent(sib_title, &refs) {
                    candidate_ids.insert(*sib_anilist_id);
                }
            }
        }
        if candidate_ids.len() == 1 {
            return Ok(Some(candidate_ids.into_iter().next().unwrap()));
        }

        Ok(None) // If siblings disagree, do not link. a wrong automatic link is worse than none.
    }

    /// Re-resolve every link that contradicts the series' own season (a
    /// "... Temporada 5" row linked to the plain season-1 entry, typically
    /// because the season-5 entry was not in the catalog yet when it linked and
    /// links were never revisited). Relinks to the right entry when one can be
    /// found, otherwise clears the link: a wrong link also makes the status sync
    /// trust the wrong season's AniList status. Idempotent. Returns how many
    /// rows changed. `link_series_to_catalog` runs this first (sharing its
    /// index); this standalone form exists for the tests.
    #[cfg(test)]
    pub fn repair_season_mislinks(&self) -> Result<usize> {
        let rows = self.catalog_titles_for_index()?;
        let index = crate::matching::CatalogIndex::build(&rows);
        let catalog_titles: std::collections::HashMap<i64, Vec<String>> =
            rows.into_iter().map(|row| (row.id, row.titles)).collect();
        self.repair_season_mislinks_with(&index, &catalog_titles)
    }

    /// Same as `repair_season_mislinks` but reuses an index the caller already
    /// built (building it over ~22k titles is the expensive part, and this
    /// runs inside a command that holds the DB mutex).
    fn repair_season_mislinks_with(
        &self,
        index: &crate::matching::CatalogIndex,
        catalog_titles: &std::collections::HashMap<i64, Vec<String>>,
    ) -> Result<usize> {
        let mut to_repair = Vec::new();
        let mut live_action_mislinks = Vec::new();

        let mut stmt = self.conn.prepare("SELECT id, title, anilist_id FROM series WHERE anilist_id IS NOT NULL")?;
        let rows = stmt.query_map([], |r| {
            let id: i64 = r.get(0)?;
            let title: String = r.get(1)?;
            let anilist_id: i64 = r.get(2)?;
            Ok((id, title, anilist_id))
        })?;

        let mut linked_series = Vec::new();
        for row in rows {
            let (id, title, anilist_id) = row?;
            linked_series.push((id, title.clone(), anilist_id));

            // A live-action adaptation linked to the anime is a different work:
            // clear it (never re-resolve, fuzzy matching would pick the anime again).
            if crate::matching::is_live_action(&title) {
                let entry_is_live = catalog_titles
                    .get(&anilist_id)
                    .is_some_and(|t| t.iter().any(|x| crate::matching::is_live_action(x)));
                if !entry_is_live {
                    live_action_mislinks.push(id);
                    continue;
                }
            }

            let norm = crate::matching::normalize_title(&title);
            let s = crate::matching::extract_season_number(&norm);
            // A colon-qualified title linked to a short show can only have got
            // there through the base-title fallback (see `is_long_runner`): keep
            // it only when its own full title matches that entry.
            if s == Some(1) && title.contains(':') && crate::db::stats::franchise_base_title(&title) != title {
                // Cheapest first. The entry's own titles are authoritative: two
                // entries can share a normalized title (a show and its movie), and
                // the index resolves that to the more popular one, which may not
                // be the one this row was (rightly) linked to.
                let own_title_matches = catalog_titles.get(&anilist_id).is_some_and(|titles| {
                    let variants = crate::matching::title_variants_norm(&title);
                    titles.iter().any(|t| variants.contains(&crate::matching::normalize_title(t)))
                });
                let justified = self.is_long_runner(anilist_id)?
                    || own_title_matches
                    || index.lookup(&[&title]) == Some(anilist_id)
                    || index.fuzzy_lookup(&[&title]) == Some(anilist_id);
                if !justified {
                    live_action_mislinks.push(id);
                    continue;
                }
            }
            if s.unwrap_or(0) >= 2 {
                // Clear only on positive evidence. An absent catalog row is
                // unknown, and so is an entry with a different subtitle ("Kaguya-sama
                // Temporada 3" -> "...: Ultra Romantic", which carries no number).
                // Evidence = the entry is the plain-titled show (same franchise key,
                // so it is season 1) or it explicitly names another season.
                let contradicts = catalog_titles.get(&anilist_id).is_some_and(|titles| {
                    let refs: Vec<&str> = titles.iter().map(String::as_str).collect();
                    if crate::matching::season_consistent(&title, &refs) {
                        return false;
                    }
                    let key = crate::matching::franchise_dedup_key(&title);
                    refs.iter().any(|t| {
                        crate::matching::franchise_dedup_key(t) == key
                            || crate::matching::extract_season_number(&crate::matching::normalize_title(t))
                                .is_some_and(|n| n >= 2)
                    })
                });
                if contradicts {
                    to_repair.push((id, title));
                }
            }
        }

        if to_repair.is_empty() && live_action_mislinks.is_empty() {
            return Ok(0);
        }

        let mut changed = 0;
        for id in live_action_mislinks {
            self.conn.execute("UPDATE series SET anilist_id = NULL WHERE id = ?1", [id])?;
            changed += 1;
        }
        let siblings = Self::sibling_index(&linked_series);
        for (id, title) in to_repair {
            if let Some(new_id) = self.resolve_catalog_link(&title, id, index, catalog_titles, &siblings)? {
                self.set_anilist_id(id, new_id)?;
            } else {
                self.conn.execute("UPDATE series SET anilist_id = NULL WHERE id = ?1", [id])?;
            }
            changed += 1;
        }

        if changed > 0 {
            eprintln!("[link] repaired {} season mislinks", changed);
        }

        Ok(changed)
    }

    pub fn link_series_to_catalog(&self) -> Result<i64> {
        let rows = self.catalog_titles_for_index()?;
        let index = crate::matching::CatalogIndex::build(&rows);
        self.link_series_to_catalog_with(&index, rows)
    }

    /// The linking pass given an already-built index. Building the index over
    /// the whole catalog is the expensive CPU part and needs no DB access, so
    /// the startup command builds it *outside* the `state.db` mutex and only
    /// holds the lock for this step; holding it for the build as well froze
    /// every data query for several seconds after each launch.
    pub fn link_series_to_catalog_with(
        &self,
        index: &crate::matching::CatalogIndex,
        rows: Vec<crate::matching::CatalogTitleRow>,
    ) -> Result<i64> {
        let catalog_titles: std::collections::HashMap<i64, Vec<String>> =
            rows.into_iter().map(|row| (row.id, row.titles)).collect();

        self.repair_season_mislinks_with(index, &catalog_titles)?;

        // Snapshot AFTER the repair so the sibling step sees the corrected links.
        let mut stmt = self.conn.prepare("SELECT id, title, anilist_id FROM series WHERE anilist_id IS NOT NULL")?;
        let linked_series: Vec<(i64, String, i64)> = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect::<rusqlite::Result<_>>()?;

        let siblings = Self::sibling_index(&linked_series);
        let mut linked = 0i64;
        for (series_id, title) in self.series_needing_catalog_link()? {
            if let Some(new_id) = self.resolve_catalog_link(&title, series_id, index, &catalog_titles, &siblings)? {
                self.set_anilist_id(series_id, new_id)?;
                linked += 1;
            }
        }
        Ok(linked)
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
    /// Capped at 500 rows and ordered linked-first, then by popularity, so
    /// shows the user actually follows/watches take priority.
    pub fn stale_status_ids(&self) -> Result<Vec<i64>> {
        let cutoff = (chrono::Utc::now() - chrono::Duration::days(120)).timestamp();
        let mut stmt = self.conn.prepare(STALE_STATUS_IDS_SQL)?;
        let ids = stmt
            .query_map([cutoff], |r| r.get::<_, i64>(0))?
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

    fn row_to_catalog_anime(r: &rusqlite::Row) -> rusqlite::Result<crate::anilist::CatalogAnime> {
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

    /// Global minimum score floor for the Descubrir catalog deck.
    /// Absent key = `None` = unbounded ("all scores") — default behavior.
    pub fn get_deck_min_score(&self) -> Result<Option<i64>> {
        Ok(self.get_setting("deck_min_score")?.and_then(|v| v.parse().ok()))
    }

    pub fn set_deck_min_score(&self, value: Option<i64>) -> Result<()> {
        match value {
            Some(v) => self.set_setting("deck_min_score", &v.to_string()),
            None => self.delete_setting("deck_min_score"),
        }
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
    ///
    /// `banned_genres` (from `get_banned_genres`) excludes any candidate that
    /// carries a banned genre *at all*, via its own `anilist_catalog_genres`
    /// rows — not just candidates whose one requested `genre` is banned. The
    /// command layer already keeps a banned genre from being picked as the
    /// round's outer genre (`filter_candidate_genres`), but that alone let a
    /// title tagged both "Accion" (allowed, and the genre this round sampled)
    /// and "Ecchi" (banned) through every filter and onto the deck — the ban
    /// list was doing half a job that the format ban list was already doing in
    /// full. Matching is `COLLATE NOCASE`, mirroring the case-insensitive
    /// comparison `filter_candidate_genres`/`banned_formats` use. An empty
    /// list adds no clause at all.
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
    // The knobs are all independent recommendation inputs; bundling them into
    // a struct would just move the same nine fields elsewhere.
    #[allow(clippy::too_many_arguments)]
    pub fn random_catalog_anime_in_genre_with_signals(
        &self,
        genre: &str,
        banned_formats: &[String],
        banned_genres: &[String],
        excluded_norm_titles: &std::collections::HashSet<String>,
        genre_affinity: &std::collections::HashMap<String, f64>,
        format_affinity: &std::collections::HashMap<String, f64>,
        taste_signals: &crate::recommend::TasteSignals,
        recommended: bool,
        hide_upcoming: bool,
        min_start_date: Option<i64>,
        max_start_date: Option<i64>,
        min_score: Option<i64>,
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
        // Same "don't punish unsynced data" policy as `upcoming_clause`: a
        // NULL `start_date` (unsynced, or synced before the column existed)
        // always passes, regardless of either bound. A non-NULL date must
        // satisfy BOTH bounds together (not either), so the min/max pair is
        // parenthesized as its own AND group, then OR'd with the NULL case.
        let mut date_clause = String::new();
        if min_start_date.is_some() || max_start_date.is_some() {
            date_clause.push_str("AND (c.start_date IS NULL OR (1=1");
            if min_start_date.is_some() {
                date_clause.push_str(" AND c.start_date >= ?");
            }
            if max_start_date.is_some() {
                date_clause.push_str(" AND c.start_date <= ?");
            }
            date_clause.push_str("))");
        }
        let mut score_clause = String::new();
        if min_score.is_some() {
            score_clause.push_str("AND c.average_score >= ?");
        }
        // A candidate carrying ANY banned genre is out, even when the genre
        // this round sampled is a perfectly allowed one — see the doc comment.
        // Blank entries are dropped so a stray empty line in the stored list
        // (`banned_genres` is a newline-joined setting) can't widen the ban.
        let banned_genres: Vec<&str> =
            banned_genres.iter().map(|g| g.trim()).filter(|g| !g.is_empty()).collect();
        let banned_genre_clause = if banned_genres.is_empty() {
            String::new()
        } else {
            let ph = banned_genres.iter().map(|_| "?").collect::<Vec<_>>().join(", ");
            format!(
                "AND c.id NOT IN (SELECT anilist_id FROM anilist_catalog_genres \
                 WHERE genre COLLATE NOCASE IN ({ph}))"
            )
        };
        let sql = format!(
            "SELECT c.id, c.title, c.title_romaji, c.title_english, c.cover_url, c.banner_url, c.format, c.episodes, c.average_score, c.popularity, c.url, c.status, c.duration, c.studio, c.start_date, c.next_airing_at, c.next_episode
             FROM anilist_catalog c
             JOIN anilist_catalog_genres g ON g.anilist_id = c.id
             WHERE g.genre = ?
               AND c.format IN ({placeholders})
               AND c.popularity >= ?
               AND c.id NOT IN (SELECT anilist_id FROM series WHERE anilist_id IS NOT NULL)
               {banned_genre_clause}
               {upcoming_clause}
               {date_clause}
               {score_clause}
             ORDER BY RANDOM() LIMIT {BATCH_SIZE}"
        );
        let mut params: Vec<Value> = vec![Value::Text(genre.to_string())];
        for f in &allowed {
            params.push(Value::Text((*f).to_string()));
        }
        params.push(Value::Integer(MIN_POPULARITY));
        // Bound in the same order the clauses appear in the SQL above: genre,
        // formats, popularity, banned genres, date bounds, then score floor.
        for g in &banned_genres {
            params.push(Value::Text((*g).to_string()));
        }
        if let Some(min) = min_start_date {
            params.push(Value::Integer(min));
        }
        if let Some(max) = max_start_date {
            params.push(Value::Integer(max));
        }
        if let Some(score) = min_score {
            params.push(Value::Integer(score));
        }
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
        // A per-id list_catalog_tags loop is acceptable when survivors <= 50.
        for anime in &mut survivors {
            anime.genres = self.list_catalog_genres(anime.id)?;
            if anime.tags.is_empty() {
                anime.tags = self.list_catalog_tags(anime.id)?;
            }
        }

        if recommended {
            let now_unix = chrono::Utc::now().timestamp();
            Ok(crate::recommend::pick_recommended(
                survivors,
                genre_affinity,
                format_affinity,
                taste_signals,
                genre,
                now_unix,
            ))
        } else {
            Ok(survivors.into_iter().next())
        }
    }

    /// Same pick without taste signals (tags/studios); the app calls `_with_signals`, tests keep this shape.
    #[cfg(test)]
    #[allow(clippy::too_many_arguments)]
    pub fn random_catalog_anime_in_genre(
        &self,
        genre: &str,
        banned_formats: &[String],
        banned_genres: &[String],
        excluded_norm_titles: &std::collections::HashSet<String>,
        genre_affinity: &std::collections::HashMap<String, f64>,
        format_affinity: &std::collections::HashMap<String, f64>,
        recommended: bool,
        hide_upcoming: bool,
        min_start_date: Option<i64>,
        max_start_date: Option<i64>,
        min_score: Option<i64>,
    ) -> Result<Option<crate::anilist::CatalogAnime>> {
        self.random_catalog_anime_in_genre_with_signals(
            genre,
            banned_formats,
            banned_genres,
            excluded_norm_titles,
            genre_affinity,
            format_affinity,
            &crate::recommend::TasteSignals::default(),
            recommended,
            hide_upcoming,
            min_start_date,
            max_start_date,
            min_score,
        )
    }

    /// Normalized-title exclusion set input: titles of `series` rows the
    /// user has already decided on — followed, "want", "discarded", or
    /// watched-externally, on ANY of the app's sites, not just the active
    /// one. See `random_catalog_anime_in_genre`'s doc comment for why this
    /// is needed alongside (not instead of) its `anilist_id NOT IN series`
    /// clause. Same site-scoping bug class already fixed for Estadísticas
    /// and cross-site progress sync — a title engaged with on site B must
    /// never keep getting re-offered by the Descubrir deck just because
    /// site A happens to be active. Returns raw titles (not yet normalized)
    /// — callers normalize via `matching::normalize_title` when building the
    /// exclusion set, keeping this function a pure DB read.
    pub fn engaged_series_titles(&self) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT title FROM series
             WHERE followed=1 OR watched_externally=1 OR backlog_status IN ('want','discarded')",
        )?;
        let titles = stmt.query_map([], |r| r.get::<_, String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(titles)
    }
}

impl Db {
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
        assert!(!db.has_synced_catalog_status().unwrap());

        db.upsert_catalog_anime(&catalog_anime_with_popularity(1, "Unsynced", &["Drama"], Some(1000)), 0).unwrap();
        assert!(!db.has_synced_catalog_status().unwrap(), "a NULL-status row alone must not count as synced");

        let mut synced = catalog_anime_with_popularity(2, "Synced", &["Drama"], Some(1000));
        synced.status = Some("RELEASING".into());
        db.upsert_catalog_anime(&synced, 1).unwrap();
        assert!(db.has_synced_catalog_status().unwrap());
    }

    /// Write a catalog row the way a *previous* build would have: straight
    /// INSERT, leaving `metadata_version` at its `DEFAULT 0`. `romaji`/`status`
    /// are parameterized so a test can reproduce the exact regression — a row
    /// that already has a romaji title (so the old `title_romaji IS NULL`
    /// marker considered it fresh) but none of the columns added after it.
    fn insert_legacy_catalog_row(db: &Db, id: i64, romaji: Option<&str>, status: Option<&str>) {
        db.conn
            .execute(
                "INSERT INTO anilist_catalog(id, title, title_romaji, url, sort_order, popularity, format, status)
                 VALUES(?1, ?2, ?3, ?4, 0, 1000, 'TV', ?5)",
                (id, format!("Legacy {id}"), romaji, format!("https://anilist.co/anime/{id}"), status),
            )
            .unwrap();
    }

    #[test]
    fn stale_catalog_ids_flags_rows_written_with_an_older_field_set() {
        let db = Db::open(":memory:").unwrap();
        // The regression: romaji present (the old marker said "fresh"), but
        // status/duration/studio/start_date never fetched.
        insert_legacy_catalog_row(&db, 1, Some("Legacy One"), None);
        // The case the old marker did catch, still caught.
        insert_legacy_catalog_row(&db, 2, None, None);

        let stale = db.stale_catalog_ids().unwrap();
        assert!(stale.contains(&1), "a romaji-but-nothing-else row must be stale: {stale:?}");
        assert!(stale.contains(&2));
    }

    #[test]
    fn stale_catalog_ids_ignores_rows_written_with_the_current_field_set() {
        let db = Db::open(":memory:").unwrap();
        // Written through the normal path, so stamped with the current version
        // — even though `studio`/`duration`/`start_date` are None here, which
        // is legitimate AniList data, not staleness.
        db.upsert_catalog_anime(&catalog_anime_with_popularity(1, "Fresh", &["Drama"], Some(1000)), 0).unwrap();
        assert!(db.stale_catalog_ids().unwrap().is_empty());
    }

    #[test]
    fn backfilling_a_stale_row_makes_it_not_stale() {
        let db = Db::open(":memory:").unwrap();
        insert_legacy_catalog_row(&db, 7, Some("Legacy Seven"), None);
        assert_eq!(db.stale_catalog_ids().unwrap(), vec![7]);

        // What `backfill_catalog_metadata` does with what it re-fetched.
        let mut refetched = catalog_anime_with_popularity(7, "Legacy 7", &["Drama"], Some(1000));
        refetched.status = Some("FINISHED".into());
        db.upsert_catalog_anime(&refetched, db.catalog_sort_order(7).unwrap()).unwrap();

        assert!(db.stale_catalog_ids().unwrap().is_empty());
    }

    #[test]
    fn metadata_version_migration_only_marks_the_genuinely_old_rows_stale() {
        // The whole point of seeding `metadata_version` at migration time:
        // defaulting every existing row to 0 would push a ~22k-row, >12h paced
        // backfill onto every user on first launch. A row that already carries
        // both a romaji title AND a status was written by a build with the
        // current field set, so it must come out non-stale; the rows the old
        // marker was blind to must come out stale.
        let path = std::env::temp_dir().join(format!(
            "aot_metaver_{}_{:?}.sqlite",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::remove_file(&path).ok();
        {
            let db = Db::open(path.to_str().unwrap()).unwrap();
            insert_legacy_catalog_row(&db, 1, Some("Has Both"), Some("FINISHED"));
            insert_legacy_catalog_row(&db, 2, Some("Romaji Only"), None);
            insert_legacy_catalog_row(&db, 3, None, None);
            // Simulate a pre-migration database: no row has been stamped yet.
            db.conn.execute("UPDATE anilist_catalog SET metadata_version = 0", []).unwrap();
        }
        // Reopening re-runs init_schema, i.e. the migration.
        let db = Db::open(path.to_str().unwrap()).unwrap();
        let stale = db.stale_catalog_ids().unwrap();
        // Legacy rows are stamped as version 1, which is behind the current
        // version (2 added synonyms), so they backfill like any other stale row.
        assert!(stale.contains(&1), "a v1-stamped row must be re-backfilled for synonyms: {stale:?}");
        assert!(stale.contains(&2), "the romaji-only row is exactly what the old marker missed");
        assert!(stale.contains(&3));
        drop(db);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn random_catalog_anime_in_genre_none_when_empty_some_when_populated() {
        let db = Db::open(":memory:").unwrap();
        assert!(db.random_catalog_anime_in_genre("Drama", &[], &[], &std::collections::HashSet::new(), &std::collections::HashMap::new(), &std::collections::HashMap::new(), true, false, None, None, None).unwrap().is_none());
        db.upsert_catalog_anime(
            &catalog_anime_with_popularity(1, "Only", &["Drama"], Some(1000)),
            0,
        )
        .unwrap();
        let picked = db.random_catalog_anime_in_genre("Drama", &[], &[], &std::collections::HashSet::new(), &std::collections::HashMap::new(), &std::collections::HashMap::new(), true, false, None, None, None).unwrap().unwrap();
        assert_eq!(picked.id, 1);
        assert_eq!(picked.genres, vec!["Drama".to_string()]);
    }

    #[test]
    fn random_catalog_anime_in_genre_only_matches_requested_genre() {
        let db = Db::open(":memory:").unwrap();
        db.upsert_catalog_anime(&catalog_anime_with_popularity(1, "Dramatic", &["Drama"], Some(1000)), 0).unwrap();
        db.upsert_catalog_anime(&catalog_anime_with_popularity(2, "Actiony", &["Action"], Some(1000)), 1).unwrap();
        let picked = db.random_catalog_anime_in_genre("Action", &[], &[], &std::collections::HashSet::new(), &std::collections::HashMap::new(), &std::collections::HashMap::new(), true, false, None, None, None).unwrap().unwrap();
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

        let picked = db.random_catalog_anime_in_genre("Drama", &[], &[], &std::collections::HashSet::new(), &std::collections::HashMap::new(), &std::collections::HashMap::new(), true, false, None, None, None).unwrap().unwrap();
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
            .random_catalog_anime_in_genre("Drama", &[], &[], &std::collections::HashSet::new(), &std::collections::HashMap::new(), &std::collections::HashMap::new(), true, true, None, None, None)
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
            .random_catalog_anime_in_genre("Drama", &[], &[], &std::collections::HashSet::new(), &std::collections::HashMap::new(), &std::collections::HashMap::new(), true, true, None, None, None)
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
            .random_catalog_anime_in_genre("Drama", &[], &[], &std::collections::HashSet::new(), &std::collections::HashMap::new(), &std::collections::HashMap::new(), true, false, None, None, None)
            .unwrap()
            .unwrap();
        assert_eq!(picked.title, "Upcoming");
    }

    #[test]
    fn random_catalog_anime_in_genre_date_range_excludes_before_min_and_after_max() {
        let db = Db::open(":memory:").unwrap();
        let mut too_old = catalog_anime_with_popularity(1, "TooOld", &["Drama"], Some(1000));
        too_old.start_date = Some(1_000);
        db.upsert_catalog_anime(&too_old, 0).unwrap();
        let mut too_new = catalog_anime_with_popularity(2, "TooNew", &["Drama"], Some(1000));
        too_new.start_date = Some(3_000);
        db.upsert_catalog_anime(&too_new, 1).unwrap();
        let mut in_range = catalog_anime_with_popularity(3, "InRange", &["Drama"], Some(1000));
        in_range.start_date = Some(2_000);
        db.upsert_catalog_anime(&in_range, 2).unwrap();

        let picked = db
            .random_catalog_anime_in_genre(
                "Drama", &[], &[], &std::collections::HashSet::new(),
                &std::collections::HashMap::new(), &std::collections::HashMap::new(),
                true, false, Some(1_500), Some(2_500), None,
            )
            .unwrap()
            .unwrap();
        assert_eq!(picked.title, "InRange");
    }

    #[test]
    fn random_catalog_anime_in_genre_date_range_keeps_unsynced_start_date_regardless_of_bounds() {
        let db = Db::open(":memory:").unwrap();
        // start_date: None (unsynced) — same "don't punish unsynced data"
        // policy as hide_upcoming's NULL-status handling.
        db.upsert_catalog_anime(&catalog_anime_with_popularity(1, "Unsynced", &["Drama"], Some(1000)), 0).unwrap();

        let picked = db
            .random_catalog_anime_in_genre(
                "Drama", &[], &[], &std::collections::HashSet::new(),
                &std::collections::HashMap::new(), &std::collections::HashMap::new(),
                true, false, Some(1_500), Some(2_500), None,
            )
            .unwrap()
            .unwrap();
        assert_eq!(picked.title, "Unsynced");
    }

    #[test]
    fn random_catalog_anime_in_genre_date_range_only_min_or_only_max_still_filters() {
        let db = Db::open(":memory:").unwrap();
        let mut low = catalog_anime_with_popularity(1, "Low", &["Drama"], Some(1000));
        low.start_date = Some(1_000);
        db.upsert_catalog_anime(&low, 0).unwrap();
        let mut high = catalog_anime_with_popularity(2, "High", &["Drama"], Some(1000));
        high.start_date = Some(3_000);
        db.upsert_catalog_anime(&high, 1).unwrap();

        // Only a min bound: excludes Low, keeps High.
        let picked = db
            .random_catalog_anime_in_genre(
                "Drama", &[], &[], &std::collections::HashSet::new(),
                &std::collections::HashMap::new(), &std::collections::HashMap::new(),
                true, false, Some(2_000), None, None,
            )
            .unwrap()
            .unwrap();
        assert_eq!(picked.title, "High");

        // Only a max bound: excludes High, keeps Low.
        let picked = db
            .random_catalog_anime_in_genre(
                "Drama", &[], &[], &std::collections::HashSet::new(),
                &std::collections::HashMap::new(), &std::collections::HashMap::new(),
                true, false, None, Some(2_000), None,
            )
            .unwrap()
            .unwrap();
        assert_eq!(picked.title, "Low");
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
            let picked = db.random_catalog_anime_in_genre("Drama", &[], &[], &std::collections::HashSet::new(), &std::collections::HashMap::new(), &std::collections::HashMap::new(), true, false, None, None, None).unwrap().unwrap();
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

        assert!(db.random_catalog_anime_in_genre("Drama", &[], &[], &std::collections::HashSet::new(), &std::collections::HashMap::new(), &std::collections::HashMap::new(), true, false, None, None, None).unwrap().is_none());
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
                .random_catalog_anime_in_genre("Drama", &["MOVIE".to_string()], &[], &std::collections::HashSet::new(), &std::collections::HashMap::new(), &std::collections::HashMap::new(), true, false, None, None, None)
                .unwrap()
                .unwrap();
            assert_eq!(picked.title, "ATVShow");
        }
    }

    #[test]
    fn random_catalog_anime_in_genre_excludes_a_candidate_carrying_a_banned_secondary_genre() {
        // The bug this covers: banning a genre only kept it from being picked
        // as the round's OUTER genre (`filter_candidate_genres`). A title
        // tagged both an allowed genre and a banned one still passed every
        // card-level filter and got served whenever the round happened to
        // sample the allowed genre.
        let db = Db::open(":memory:").unwrap();
        db.upsert_catalog_anime(
            &catalog_anime_with_popularity(1, "AlsoEcchi", &["Accion", "Ecchi"], Some(1000)),
            0,
        )
        .unwrap();
        db.upsert_catalog_anime(
            &catalog_anime_with_popularity(2, "CleanPick", &["Accion"], Some(1000)),
            1,
        )
        .unwrap();

        // Queried under the ALLOWED genre — the banned one is only a secondary
        // tag on the candidate, which is exactly what used to slip through.
        for _ in 0..10 {
            let picked = db
                .random_catalog_anime_in_genre(
                    "Accion",
                    &[],
                    &["Ecchi".to_string()],
                    &std::collections::HashSet::new(),
                    &std::collections::HashMap::new(),
                    &std::collections::HashMap::new(),
                    true,
                    false,
                    None,
                    None,
                    None,
                )
                .unwrap()
                .unwrap();
            assert_eq!(picked.title, "CleanPick");
        }
    }

    #[test]
    fn random_catalog_anime_in_genre_banned_genre_match_is_case_insensitive_and_can_empty_the_deck() {
        // Case-insensitive, mirroring `filter_candidate_genres`'s
        // `eq_ignore_ascii_case` — the stored ban list is user-entered text.
        let db = Db::open(":memory:").unwrap();
        db.upsert_catalog_anime(
            &catalog_anime_with_popularity(1, "AlsoEcchi", &["Accion", "Ecchi"], Some(1000)),
            0,
        )
        .unwrap();

        assert!(db
            .random_catalog_anime_in_genre(
                "Accion",
                &[],
                &["ecchi".to_string()],
                &std::collections::HashSet::new(),
                &std::collections::HashMap::new(),
                &std::collections::HashMap::new(),
                true,
                false,
                None,
                None,
                None,
            )
            .unwrap()
            .is_none());

        // Control: with no ban list at all the same title is returnable, so
        // the exclusion above is the ban doing it, not some other filter.
        assert!(db
            .random_catalog_anime_in_genre(
                "Accion",
                &[],
                &[],
                &std::collections::HashSet::new(),
                &std::collections::HashMap::new(),
                &std::collections::HashMap::new(),
                true,
                false,
                None,
                None,
                None,
            )
            .unwrap()
            .is_some());
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
            .random_catalog_anime_in_genre("Drama", &all_banned, &[], &std::collections::HashSet::new(), &std::collections::HashMap::new(), &std::collections::HashMap::new(), true, false, None, None, None)
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
            db.engaged_series_titles().unwrap().iter().map(|t| crate::matching::normalize_title(t)).collect();
        assert!(excluded.contains(&crate::matching::normalize_title("Overlord IV")));

        for _ in 0..10 {
            let picked = db.random_catalog_anime_in_genre("Fantasy", &[], &[], &excluded, &std::collections::HashMap::new(), &std::collections::HashMap::new(), true, false, None, None, None).unwrap().unwrap();
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
            db.engaged_series_titles().unwrap().iter().map(|t| crate::matching::normalize_title(t)).collect();
        assert!(db.random_catalog_anime_in_genre("Fantasy", &[], &[], &excluded, &std::collections::HashMap::new(), &std::collections::HashMap::new(), true, false, None, None, None).unwrap().is_none());
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
        // ~90% win rate per pick; 200 draws and a 150 floor sit ~7 sigma
        // below the mean, so this cannot flake (25/30 did, ~1 run in 12).
        for _ in 0..200 {
            let picked = db
                .random_catalog_anime_in_genre(
                    "Drama",
                    &[],
                    &[],
                    &std::collections::HashSet::new(),
                    &genre_affinity,
                    &format_affinity,
                    true,
                    false,
                    None,
                    None,
                    None,
                )
                .unwrap()
                .unwrap();
            if picked.title == "TasteMatch" {
                taste_match_wins += 1;
            }
        }
        assert!(
            taste_match_wins >= 150,
            "expected the secondary-genre-matching candidate to dominate the weighted pick, won {taste_match_wins}/200"
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
                    &[],
                    &std::collections::HashSet::new(),
                    &genre_affinity,
                    &format_affinity,
                    false,
                    false,
                    None,
                    None,
                    None,
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
            db.engaged_series_titles().unwrap().iter().map(|t| crate::matching::normalize_title(t)).collect();

        for _ in 0..10 {
            let picked = db
                .random_catalog_anime_in_genre(
                    "Fantasy",
                    &[],
                    &[],
                    &excluded,
                    &std::collections::HashMap::new(),
                    &std::collections::HashMap::new(),
                    false,
                    false,
                    None,
                    None,
                    None,
                )
                .unwrap()
                .unwrap();
            assert_eq!(picked.title, "Some Other Show");
        }
    }

    #[test]
    fn link_series_to_catalog_links_an_exact_case_insensitive_title_match() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("GogoAnime", "g", "gogoanime").unwrap();
        db.upsert_catalog_anime(&catalog_anime(187538, "BLACK TORCH", &["Action"]), 0).unwrap();

        let sid = db.upsert_series(src, &crate::models::Series {
            id: 0, slug: "black-torch".into(), title: "Black Torch".into(),
            url: "https://gogoanime.example/series/black-torch".into(),
            cover_url: None, is_airing: true, followed: true,
            next_episode_at: None, site_episode_count: None,
        }).unwrap();
        db.set_followed(sid, true).unwrap();

        let linked = db.link_series_to_catalog().unwrap();
        assert_eq!(linked, 1);
        assert_eq!(
            db.conn.query_row("SELECT anilist_id FROM series WHERE id=?1", [sid], |r| r.get::<_, Option<i64>>(0)).unwrap(),
            Some(187538)
        );
    }

    #[test]
    fn link_series_to_catalog_prefers_the_matching_season_over_season_one() {
        // The reported bug: a site's "... Temporada 2" title has no exact
        // catalog hit (AniList stores "... 2nd Season"), and its franchise
        // base ("...", season markers stripped) exact-matches season 1 —
        // which used to win outright before the full title's fuzzy score
        // against the real season-2 entry was ever tried. Season 1 is
        // seeded with far higher popularity so a popularity tie-break bug
        // would also surface this as a failure.
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("TioAnime", "t", "tioanime").unwrap();
        db.upsert_catalog_anime(
            &catalog_anime_with_popularity(100, "Seihantai na Kimi to Boku", &["Comedy"], Some(50_000)),
            0,
        ).unwrap();
        db.upsert_catalog_anime(
            &catalog_anime_with_popularity(200, "Seihantai na Kimi to Boku 2nd Season", &["Comedy"], Some(5_000)),
            0,
        ).unwrap();

        let sid = db.upsert_series(src, &crate::models::Series {
            id: 0, slug: "seihantai-2".into(), title: "Seihantai na Kimi to Boku Temporada 2".into(),
            url: "https://tioanime.example/series/seihantai-2".into(),
            cover_url: None, is_airing: true, followed: true,
            next_episode_at: None, site_episode_count: None,
        }).unwrap();
        db.set_followed(sid, true).unwrap();

        assert_eq!(db.link_series_to_catalog().unwrap(), 1);
        assert_eq!(
            db.conn.query_row("SELECT anilist_id FROM series WHERE id=?1", [sid], |r| r.get::<_, Option<i64>>(0)).unwrap(),
            Some(200),
            "must link to season 2's own entry, not season 1's"
        );
    }

    #[test]
    fn link_series_to_catalog_falls_back_to_the_franchise_base_for_an_arc_with_no_entry_of_its_own() {
        // An arc split ("One Piece: Arco de Elbaph") has no AniList entry of
        // its own — the full title correctly finds nothing (exact or
        // fuzzy), so this must still fall back to the parent show's entry.
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("TioAnime", "t", "tioanime").unwrap();
        // One Piece airs: AniList has no episode total yet (None), i.e. a long-runner.
        let mut op = catalog_anime(21, "One Piece", &["Action"]);
        op.episodes = None;
        db.upsert_catalog_anime(&op, 0).unwrap();

        let sid = db.upsert_series(src, &crate::models::Series {
            id: 0, slug: "one-piece-elbaph".into(), title: "One Piece: Arco de Elbaph".into(),
            url: "https://tioanime.example/series/one-piece-elbaph".into(),
            cover_url: None, is_airing: true, followed: true,
            next_episode_at: None, site_episode_count: None,
        }).unwrap();
        db.set_followed(sid, true).unwrap();

        assert_eq!(db.link_series_to_catalog().unwrap(), 1);
        assert_eq!(
            db.conn.query_row("SELECT anilist_id FROM series WHERE id=?1", [sid], |r| r.get::<_, Option<i64>>(0)).unwrap(),
            Some(21)
        );
    }

    fn series_with(db: &Db, src: i64, slug: &str, title: &str) -> i64 {
        let sid = db.upsert_series(src, &crate::models::Series {
            id: 0, slug: slug.into(), title: title.into(),
            url: format!("https://example/series/{slug}"),
            cover_url: None, is_airing: true, followed: true,
            next_episode_at: None, site_episode_count: None,
        }).unwrap();
        db.set_followed(sid, true).unwrap();
        sid
    }

    fn linked_id(db: &Db, sid: i64) -> Option<i64> {
        db.conn.query_row("SELECT anilist_id FROM series WHERE id=?1", [sid], |r| r.get(0)).unwrap()
    }

    #[test]
    fn link_never_falls_back_to_season_one_for_a_season_n_title() {
        // Real bug: animeytx "Kanojo, Okarishimasu Temporada 5" got linked to the
        // plain season-1 entry (FINISHED), so a show airing now was marked finished.
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "a", "animeytx").unwrap();
        db.upsert_catalog_anime(&catalog_anime(113813, "Kanojo, Okarishimasu", &[]), 0).unwrap();
        let s5 = series_with(&db, src, "k5", "Kanojo, Okarishimasu Temporada 5");
        let s1 = series_with(&db, src, "k1", "Kanojo, Okarishimasu");

        db.link_series_to_catalog().unwrap();
        assert_eq!(linked_id(&db, s5), None, "a season-5 title must stay unlinked rather than join season 1");
        assert_eq!(linked_id(&db, s1), Some(113813), "the plain title still links to season 1");
    }

    #[test]
    fn link_does_not_attach_a_live_action_title_to_the_anime() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "a", "animeytx").unwrap();
        let mut op = catalog_anime(21, "One Piece", &[]);
        op.episodes = None;
        db.upsert_catalog_anime(&op, 0).unwrap();
        let live = series_with(&db, src, "op-live", "One Piece: Live Action (2023)");
        let arc = series_with(&db, src, "op-arc", "One Piece: Arco de Elbaph");

        db.link_series_to_catalog().unwrap();
        assert_eq!(linked_id(&db, live), None, "live action is a different work");
        assert_eq!(linked_id(&db, arc), Some(21), "arcs still fall back to the parent show");
    }

    #[test]
    fn repair_moves_a_wrong_season_link_to_the_right_entry_once_it_exists() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "a", "animeytx").unwrap();
        db.upsert_catalog_anime(&catalog_anime(113813, "Kanojo, Okarishimasu", &[]), 0).unwrap();
        let s5 = series_with(&db, src, "k5", "Kanojo, Okarishimasu Temporada 5");
        db.set_anilist_id(s5, 113813).unwrap(); // the stale wrong link

        // Season 5 enters the catalog later.
        db.upsert_catalog_anime(&catalog_anime(199029, "Kanojo, Okarishimasu 5th Season", &[]), 1).unwrap();

        assert_eq!(db.repair_season_mislinks().unwrap(), 1);
        assert_eq!(linked_id(&db, s5), Some(199029));
        assert_eq!(db.repair_season_mislinks().unwrap(), 0, "idempotent");
    }

    #[test]
    fn repair_unlinks_a_live_action_row_attached_to_the_anime() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "a", "animeytx").unwrap();
        db.upsert_catalog_anime(&catalog_anime(21, "One Piece", &[]), 0).unwrap();
        let live = series_with(&db, src, "op-live", "One Piece: Live Action");
        db.set_anilist_id(live, 21).unwrap();
        let anime = series_with(&db, src, "op", "One Piece");
        db.set_anilist_id(anime, 21).unwrap();

        assert_eq!(db.repair_season_mislinks().unwrap(), 1);
        assert_eq!(linked_id(&db, live), None);
        assert_eq!(linked_id(&db, anime), Some(21));
    }

    #[test]
    fn an_ambiguous_final_season_title_never_joins_season_one() {
        let db = Db::open(":memory:").unwrap();
        let a = db.upsert_source("AnimeYT", "a", "animeytx").unwrap();
        let b = db.upsert_source("Gogo", "g", "gogoanime").unwrap();
        db.upsert_catalog_anime(&catalog_anime(16498, "Attack on Titan", &[]), 0).unwrap();
        let plain = series_with(&db, a, "aot", "Attack on Titan");
        let fin = series_with(&db, b, "aot-final", "Attack on Titan The Final Season");

        db.link_series_to_catalog().unwrap();
        assert_eq!(linked_id(&db, plain), Some(16498));
        assert_eq!(linked_id(&db, fin), None, "via base fallback or sibling it would join season 1");
    }

    #[test]
    fn repair_keeps_a_link_whose_catalog_row_is_absent_or_final_season() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "a", "animeytx").unwrap();
        db.upsert_catalog_anime(&catalog_anime(110277, "Attack on Titan: The Final Season", &[]), 0).unwrap();
        let s4 = series_with(&db, src, "aot4", "Shingeki no Kyojin Temporada 4");
        db.set_anilist_id(s4, 110277).unwrap();
        let orphan = series_with(&db, src, "x3", "Some Show Temporada 3");
        db.set_anilist_id(orphan, 999_999).unwrap(); // catalog row not loaded

        assert_eq!(db.repair_season_mislinks().unwrap(), 0);
        assert_eq!(linked_id(&db, s4), Some(110277));
        assert_eq!(linked_id(&db, orphan), Some(999_999));
    }

    #[test]
    fn repair_keeps_a_link_to_a_subtitled_sequel_entry() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "a", "animeytx").unwrap();
        db.upsert_catalog_anime(&catalog_anime(101921, "Kaguya-sama: Ultra Romantic", &[]), 0).unwrap();
        let s3 = series_with(&db, src, "k3", "Kaguya-sama wa Kokurasetai Temporada 3");
        db.set_anilist_id(s3, 101921).unwrap();

        assert_eq!(db.repair_season_mislinks().unwrap(), 0);
        assert_eq!(linked_id(&db, s3), Some(101921));
    }

    #[test]
    fn a_colon_subtitle_never_inherits_a_short_shows_link() {
        // Real bug: "Kami no Tou: Ouji no Kikan" (season 2) was linked to the
        // season-1 entry through the colon-stripped base title.
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "a", "animeytx").unwrap();
        let mut tog = catalog_anime(115230, "Tower of God", &[]);
        tog.title_romaji = Some("Kami no Tou: Tower of God".into());
        tog.synonyms = vec!["Kami no Tou".into()];
        db.upsert_catalog_anime(&tog, 0).unwrap();
        let s2 = series_with(&db, src, "tog2", "Kami no Tou: Ouji no Kikan");

        db.link_series_to_catalog().unwrap();
        assert_eq!(linked_id(&db, s2), None);
    }

    #[test]
    fn repair_unlinks_a_colon_subtitle_that_was_linked_through_the_base_title() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "a", "animeytx").unwrap();
        db.upsert_catalog_anime(&catalog_anime(115230, "Tower of God", &[]), 0).unwrap(); // 12 episodes
        let s2 = series_with(&db, src, "tog2", "Tower of God: Ouji no Kikan");
        db.set_anilist_id(s2, 115230).unwrap();
        let mut op = catalog_anime(21, "One Piece", &[]);
        op.episodes = None;
        db.upsert_catalog_anime(&op, 1).unwrap();
        let arc = series_with(&db, src, "op-arc", "One Piece: Arco de Elbaph");
        db.set_anilist_id(arc, 21).unwrap();

        assert_eq!(db.repair_season_mislinks().unwrap(), 1);
        assert_eq!(linked_id(&db, s2), None);
        assert_eq!(linked_id(&db, arc), Some(21), "arcs of a long-runner keep the parent link");
    }

    #[test]
    fn repair_keeps_a_colon_row_linked_to_an_entry_with_the_same_title_even_when_a_duplicate_is_more_popular() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "a", "animeytx").unwrap();
        // A series and its movie share the exact title; the movie is more popular.
        let mut tv = catalog_anime(1, "Foo: The Story", &[]);
        tv.popularity = Some(10);
        let mut movie = catalog_anime(2, "Foo: The Story", &[]);
        movie.popularity = Some(9000);
        db.upsert_catalog_anime(&tv, 0).unwrap();
        db.upsert_catalog_anime(&movie, 1).unwrap();
        let row = series_with(&db, src, "foo", "Foo: The Story");
        db.set_anilist_id(row, 1).unwrap();
        // and a row linked through its parenthetical spelling
        let mut inner = catalog_anime(3, "Baz Qux", &[]);
        inner.popularity = Some(5);
        db.upsert_catalog_anime(&inner, 2).unwrap();
        let paren = series_with(&db, src, "bq", "Foo: Bar (Baz Qux)");
        db.set_anilist_id(paren, 3).unwrap();

        assert_eq!(db.repair_season_mislinks().unwrap(), 0);
        assert_eq!(linked_id(&db, row), Some(1));
        assert_eq!(linked_id(&db, paren), Some(3));
    }

    #[test]
    fn repair_clears_a_wrong_season_link_when_no_right_entry_exists() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "a", "animeytx").unwrap();
        db.upsert_catalog_anime(&catalog_anime(113813, "Kanojo, Okarishimasu", &[]), 0).unwrap();
        let s5 = series_with(&db, src, "k5", "Kanojo, Okarishimasu Temporada 5");
        db.set_anilist_id(s5, 113813).unwrap();

        assert_eq!(db.repair_season_mislinks().unwrap(), 1);
        assert_eq!(linked_id(&db, s5), None, "a wrong link is worse than none");
        // and a correct link is never touched
        let ok = series_with(&db, src, "k1", "Kanojo, Okarishimasu");
        db.set_anilist_id(ok, 113813).unwrap();
        assert_eq!(db.repair_season_mislinks().unwrap(), 0);
        assert_eq!(linked_id(&db, ok), Some(113813));
    }

    #[test]
    fn a_sibling_on_another_site_with_the_same_season_lends_its_link() {
        // Site A's title matches the catalog entry; site B's spelling of the SAME
        // season matches nothing by itself, but shares franchise key and season.
        let db = Db::open(":memory:").unwrap();
        let a = db.upsert_source("AnimeYT", "a", "animeytx").unwrap();
        let b = db.upsert_source("Gogo", "g", "gogoanime").unwrap();
        db.upsert_catalog_anime(&catalog_anime(500, "Pretty Name 2nd Season", &[]), 0).unwrap();
        let site_a = series_with(&db, a, "pn-a", "Pretty Name 2nd Season");
        db.set_anilist_id(site_a, 500).unwrap();
        let site_b = series_with(&db, b, "pn-b", "Pretty Name Temporada 2");
        // a different season of the same franchise must NOT inherit it
        let other = series_with(&db, b, "pn-b3", "Pretty Name Temporada 3");

        db.link_series_to_catalog().unwrap();
        assert_eq!(linked_id(&db, site_b), Some(500));
        assert_eq!(linked_id(&db, other), None, "season 3 must not borrow season 2's link");
    }

    #[test]
    fn link_series_to_catalog_leaves_a_title_with_no_catalog_match_unlinked() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("GogoAnime", "g", "gogoanime").unwrap();
        let sid = db.upsert_series(src, &crate::models::Series {
            id: 0, slug: "totally-obscure".into(), title: "Totally Obscure Show Nobody Synced".into(),
            url: "https://gogoanime.example/series/totally-obscure".into(),
            cover_url: None, is_airing: true, followed: true,
            next_episode_at: None, site_episode_count: None,
        }).unwrap();
        db.set_followed(sid, true).unwrap();

        assert_eq!(db.link_series_to_catalog().unwrap(), 0);
        assert_eq!(
            db.conn.query_row("SELECT anilist_id FROM series WHERE id=?1", [sid], |r| r.get::<_, Option<i64>>(0)).unwrap(),
            None
        );
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
            synonyms: Vec::new(), next_airing_at: None, next_episode: None, tags: Vec::new(), relations: Vec::new(),
            cover_url: None,
            banner_url: None,
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
            title_english: None, synonyms: Vec::new(), next_airing_at: None, next_episode: None, tags: Vec::new(), relations: Vec::new(),
            cover_url: None,
            banner_url: None,
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
            title_english: None, synonyms: Vec::new(), next_airing_at: None, next_episode: None, tags: Vec::new(), relations: Vec::new(),
            cover_url: None,
            banner_url: None,
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
            title_english: None, synonyms: Vec::new(),
            cover_url: None,
            banner_url: None,
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
            next_airing_at: None, next_episode: None, tags: Vec::new(), relations: Vec::new(),
        };
        db.upsert_catalog_anime(&with_studio, 0).unwrap();

        // A row that never sets studio (mirrors a pre-existing/unsynced row,
        // or a title AniList credits no studio for) — must default to None,
        // not error or coerce to an empty string.
        let without_studio = crate::anilist::CatalogAnime {
            id: 46,
            title: "No Studio Show".into(),
            title_romaji: None,
            title_english: None, synonyms: Vec::new(),
            cover_url: None,
            banner_url: None,
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
            next_airing_at: None, next_episode: None, tags: Vec::new(), relations: Vec::new(),
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
            title_english: None, synonyms: Vec::new(),
            cover_url: None,
            banner_url: None,
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
            next_airing_at: None, next_episode: None, tags: Vec::new(), relations: Vec::new(),
        };
        db.upsert_catalog_anime(&with_date, 0).unwrap();

        let without_date = crate::anilist::CatalogAnime {
            id: 48,
            title: "Undated Show".into(),
            title_romaji: None,
            title_english: None, synonyms: Vec::new(),
            cover_url: None,
            banner_url: None,
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
            next_airing_at: None, next_episode: None, tags: Vec::new(), relations: Vec::new(),
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
                title_english: Some("Attack on Titan".into()), synonyms: Vec::new(), cover_url: None, banner_url: None, format: Some("TV".into()),
                genres: vec![], episodes: Some(25), average_score: None, popularity: None,
                url: "https://anilist.co/anime/50".into(), status: None, duration: None, studio: None,
                start_date: Some(1_776_211_200),
                next_airing_at: None, next_episode: None, tags: Vec::new(), relations: Vec::new(),
            },
            0,
        ).unwrap();

        // No start_date synced yet — must be entirely absent from the map,
        // not present with a None/0 value.
        db.upsert_catalog_anime(
            &crate::anilist::CatalogAnime {
                id: 51, title: "Unsynced Show".into(), title_romaji: None, title_english: None, synonyms: Vec::new(),
                cover_url: None, banner_url: None, format: Some("TV".into()), genres: vec![], episodes: None,
                average_score: None, popularity: None, url: "https://anilist.co/anime/51".into(),
                status: None, duration: None, studio: None, start_date: None,
                next_airing_at: None, next_episode: None, tags: Vec::new(), relations: Vec::new(),
            },
            1,
        ).unwrap();

        let map = db.catalog_start_dates_by_normalized_title().unwrap();
        assert_eq!(map.get(&crate::matching::normalize_title("Attack on Titan")), Some(&1_776_211_200));
        assert_eq!(map.get(&crate::matching::normalize_title("Shingeki no Kyojin")), Some(&1_776_211_200));
        assert_eq!(map.get(&crate::matching::normalize_title("Unsynced Show")), None);
        assert_eq!(map.len(), 2, "one entry per distinct title/romaji/english variant with a real start_date");
    }

    #[test]
    fn upsert_catalog_anime_stores_and_rewrites_relations() {
        let db = Db::open(":memory:").unwrap();

        let mut anime = crate::anilist::CatalogAnime {
            id: 99,
            title: "Relations Show".into(),
            title_romaji: None,
            title_english: None,
            synonyms: Vec::new(),
            next_airing_at: None,
            next_episode: None,
            tags: Vec::new(),
            relations: vec![
                crate::anilist::CatalogRelation { related_id: 100, relation_type: "SEQUEL".into() },
                crate::anilist::CatalogRelation { related_id: 98, relation_type: "PREQUEL".into() },
            ],
            cover_url: None,
            banner_url: None,
            format: Some("TV".into()),
            genres: vec![],
            episodes: Some(12),
            average_score: None,
            popularity: None,
            url: "https://anilist.co/anime/99".into(),
            status: None,
            duration: None,
            studio: None,
            start_date: None,
        };
        db.upsert_catalog_anime(&anime, 0).unwrap();

        let stored = db.list_catalog_relations(99).unwrap();
        assert_eq!(
            stored,
            vec![(98, "PREQUEL".to_string()), (100, "SEQUEL".to_string())]
        );

        // Second upsert with fewer relations leaves only the new set.
        anime.relations = vec![
            crate::anilist::CatalogRelation { related_id: 100, relation_type: "SEQUEL".into() },
        ];
        db.upsert_catalog_anime(&anime, 0).unwrap();

        let updated = db.list_catalog_relations(99).unwrap();
        assert_eq!(updated, vec![(100, "SEQUEL".to_string())]);
    }

    fn mk_catalog_anime(id: i64, title: &str, url: &str) -> crate::anilist::CatalogAnime {
        crate::anilist::CatalogAnime {
            id, title: title.into(), title_romaji: None, title_english: None, synonyms: Vec::new(), cover_url: None,
            banner_url: None, format: Some("TV".into()), genres: vec![], episodes: Some(12), average_score: None,
            popularity: None, url: url.into(), status: None, duration: None, studio: None, start_date: None,
            next_airing_at: None, next_episode: None, tags: Vec::new(), relations: Vec::new(),
        }
    }

    #[test]
    fn catalog_info_for_series_uses_direct_anilist_id_when_linked() {
        let db = Db::open(":memory:").unwrap();
        db.upsert_catalog_anime(&mk_catalog_anime(70, "Frieren", "https://anilist.co/anime/70"), 0).unwrap();
        let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
        let sid = db.upsert_series(src, &mk_airing("frieren-site", "Some Other Scraped Title", None)).unwrap();
        db.set_anilist_id(sid, 70).unwrap();

        // Direct id join wins even though the scraped title wouldn't
        // title-match — proves it doesn't fall through to the title lookup.
        let info = db.catalog_info_for_series(sid).unwrap().expect("linked match");
        assert_eq!(info.url, "https://anilist.co/anime/70");
    }

    #[test]
    fn catalog_info_for_series_falls_back_to_normalized_title_match_when_unlinked() {
        let db = Db::open(":memory:").unwrap();
        db.upsert_catalog_anime(&mk_catalog_anime(71, "Bocchi the Rock!", "https://anilist.co/anime/71"), 0).unwrap();
        let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
        // Followed/scraped series never get anilist_id set in practice — no
        // set_anilist_id call here, matching real-world coverage.
        let sid = db.upsert_series(src, &mk_airing("bocchi", "Bocchi The Rock! Sub Español", None)).unwrap();

        let info = db.catalog_info_for_series(sid).unwrap().expect("title match");
        assert_eq!(info.url, "https://anilist.co/anime/71");
    }

    #[test]
    fn catalog_info_for_series_ignores_a_dangling_anilist_id() {
        // anilist_id points at a row that was never synced (or since removed)
        // — must fall through to the title-match path instead of erroring.
        let db = Db::open(":memory:").unwrap();
        db.upsert_catalog_anime(&mk_catalog_anime(72, "Real Match", "https://anilist.co/anime/72"), 0).unwrap();
        let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
        let sid = db.upsert_series(src, &mk_airing("real-match", "Real Match", None)).unwrap();
        db.set_anilist_id(sid, 9999).unwrap(); // never synced

        let info = db.catalog_info_for_series(sid).unwrap().expect("falls through to title match");
        assert_eq!(info.url, "https://anilist.co/anime/72");
    }

    fn mk_full_catalog_anime(id: i64, title: &str, genres: &[&str]) -> crate::anilist::CatalogAnime {
        crate::anilist::CatalogAnime {
            id, title: title.into(), title_romaji: None, title_english: None, synonyms: Vec::new(),
            cover_url: Some(format!("https://img/{id}")), banner_url: None, format: Some("TV".into()),
            genres: genres.iter().map(|g| g.to_string()).collect(), episodes: Some(24),
            average_score: Some(88), popularity: Some(5000), url: format!("https://anilist.co/anime/{id}"),
            status: Some("FINISHED".into()), duration: Some(24), studio: Some("Studio X".into()),
            start_date: None,
            next_airing_at: None, next_episode: None, tags: Vec::new(), relations: Vec::new(),
        }
    }

    #[test]
    fn catalog_info_for_series_returns_full_matched_row_with_genres() {
        let db = Db::open(":memory:").unwrap();
        let anime = mk_full_catalog_anime(80, "Frieren", &["Adventure", "Drama"]);
        db.upsert_catalog_anime(&anime, 0).unwrap();
        let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
        let sid = db.upsert_series(src, &mk_airing("frieren", "Frieren Sub Español", None)).unwrap();

        let info = db.catalog_info_for_series(sid).unwrap().expect("should title-match");
        assert_eq!(info.id, 80);
        assert_eq!(info.studio.as_deref(), Some("Studio X"));
        assert_eq!(info.average_score, Some(88));
        assert_eq!(info.genres, vec!["Adventure".to_string(), "Drama".to_string()]);
    }

    #[test]
    fn catalog_info_for_series_none_when_unmatched() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "b", "animeytx").unwrap();
        let sid = db.upsert_series(src, &mk_airing("obscure", "Totally Unmatched Title", None)).unwrap();

        assert_eq!(db.catalog_info_for_series(sid).unwrap(), None);
    }

    #[test]
    fn link_series_to_catalog_links_squashed_spacing_and_punctuation_variants() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("TioAnime", "t", "tioanime").unwrap();

        // Seed catalog with target titles differing only by punctuation / whitespace
        db.upsert_catalog_anime(&catalog_anime(184356, "Dogul Wang", &["Action"]), 0).unwrap();
        db.upsert_catalog_anime(
            &catalog_anime(199066, "Hanaori-san wa Tensei Shite mo Kenka ga Shitai", &["Romance"]),
            1,
        )
        .unwrap();
        db.upsert_catalog_anime(&catalog_anime(300, "Let's Go Kaiki-gumi", &["Comedy"]), 2).unwrap();
        db.upsert_catalog_anime(&catalog_anime(400, "Hime Kishi wa Barbaroi no Yome", &["Fantasy"]), 3).unwrap();
        db.upsert_catalog_anime(&catalog_anime(500, "Ichijouma Mankitsu Gurashi!", &["Slice of Life"]), 4).unwrap();
        db.upsert_catalog_anime(
            &catalog_anime(600, "Taiari deshita: Ojousama wa Kakutou Game nante Shinai", &["Comedy"]),
            5,
        )
        .unwrap();

        let test_cases = [
            ("dogulwang", "Dogulwang", 184356),
            ("hanaori", "Hanaori-san wa Tensei shitemo Kenka ga Shitai", 199066),
            ("kaiki", "Let’s Go Kaiki-gumi", 300), // curly apostrophe
            ("himekishi", "Himekishi wa Barbaroi no Yome", 400),
            ("ichijouma", "Ichijouma Mankitsugurashi!", 500),
            ("taiari", "Tai-Ari deshita. Ojousama wa Kakutou Game nante Shinai", 600),
        ];

        let mut sids = Vec::new();
        for (slug, title, _) in &test_cases {
            let sid = db
                .upsert_series(
                    src,
                    &crate::models::Series {
                        id: 0,
                        slug: (*slug).into(),
                        title: (*title).into(),
                        url: format!("https://tioanime.example/series/{slug}"),
                        cover_url: None,
                        is_airing: true,
                        followed: true,
                        next_episode_at: None,
                        site_episode_count: None,
                    },
                )
                .unwrap();
            db.set_followed(sid, true).unwrap();
            sids.push(sid);
        }

        let linked = db.link_series_to_catalog().unwrap();
        assert_eq!(linked, 6);

        for (sid, (_, _, expected_id)) in sids.iter().zip(test_cases.iter()) {
            let actual_id = db
                .conn
                .query_row(
                    "SELECT anilist_id FROM series WHERE id=?1",
                    [sid],
                    |r| r.get::<_, Option<i64>>(0),
                )
                .unwrap();
            assert_eq!(actual_id, Some(*expected_id));
        }
    }

    #[test]
    fn link_series_to_catalog_short_title_stays_unlinked() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("TioAnime", "t", "tioanime").unwrap();
        db.upsert_catalog_anime(&catalog_anime(700, "Hero", &["Action"]), 0).unwrap();

        // "He ro" squashes to "hero" (4 chars < 6), must not match
        let sid = db
            .upsert_series(
                src,
                &crate::models::Series {
                    id: 0,
                    slug: "he-ro".into(),
                    title: "He ro".into(),
                    url: "https://tioanime.example/series/he-ro".into(),
                    cover_url: None,
                    is_airing: true,
                    followed: true,
                    next_episode_at: None,
                    site_episode_count: None,
                },
            )
            .unwrap();
        db.set_followed(sid, true).unwrap();

        let linked = db.link_series_to_catalog().unwrap();
        assert_eq!(linked, 0);
        let actual_id = db
            .conn
            .query_row(
                "SELECT anilist_id FROM series WHERE id=?1",
                [sid],
                |r| r.get::<_, Option<i64>>(0),
            )
            .unwrap();
        assert_eq!(actual_id, None, "short squashed key (< 6 chars) must stay unlinked");
    }

    #[test]
    fn stale_status_ids_selects_airing_linked_releasing_and_recent_finished() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("TestSite", "https://site.example", "testsite").unwrap();
        let now = chrono::Utc::now().timestamp();

        // 1. Linked to a series with is_airing=1 (even if status is FINISHED and start_date is old)
        let mut anime1 = catalog_anime_with_popularity(1, "AiringLinked", &[], Some(100));
        anime1.status = Some("FINISHED".into());
        anime1.start_date = Some(now - 300 * 86400); // 300 days ago
        db.upsert_catalog_anime(&anime1, 0).unwrap();
        let mut s1 = mk_airing("s1", "AiringLinked", None);
        s1.is_airing = true;
        let sid1 = db.upsert_series(src, &s1).unwrap();
        db.set_anilist_id(sid1, 1).unwrap();

        // 2. Unlinked RELEASING
        let mut anime2 = catalog_anime_with_popularity(2, "Releasing", &[], Some(200));
        anime2.status = Some("RELEASING".into());
        db.upsert_catalog_anime(&anime2, 1).unwrap();

        // 3. Unlinked NOT_YET_RELEASED
        let mut anime3 = catalog_anime_with_popularity(3, "Upcoming", &[], Some(300));
        anime3.status = Some("NOT_YET_RELEASED".into());
        db.upsert_catalog_anime(&anime3, 2).unwrap();

        // 4. Unlinked FINISHED within last 120 days (e.g. 60 days ago)
        let mut anime4 = catalog_anime_with_popularity(4, "RecentFinished", &[], Some(400));
        anime4.status = Some("FINISHED".into());
        anime4.start_date = Some(now - 60 * 86400);
        db.upsert_catalog_anime(&anime4, 3).unwrap();

        // 5. Excluded: Unlinked FINISHED older than 120 days (e.g. 150 days ago)
        let mut anime5 = catalog_anime_with_popularity(5, "OldFinished", &[], Some(500));
        anime5.status = Some("FINISHED".into());
        anime5.start_date = Some(now - 150 * 86400);
        db.upsert_catalog_anime(&anime5, 4).unwrap();

        // 6. Excluded: Linked to series with is_airing=0, FINISHED and old start_date
        let mut anime6 = catalog_anime_with_popularity(6, "NonAiringLinkedOld", &[], Some(600));
        anime6.status = Some("FINISHED".into());
        anime6.start_date = Some(now - 200 * 86400);
        db.upsert_catalog_anime(&anime6, 5).unwrap();
        let mut s6 = mk_airing("s6", "NonAiringLinkedOld", None);
        s6.is_airing = false;
        let sid6 = db.upsert_series(src, &s6).unwrap();
        db.set_anilist_id(sid6, 6).unwrap();

        // 7. Excluded: Unlinked CANCELLED
        let mut anime7 = catalog_anime_with_popularity(7, "Cancelled", &[], Some(700));
        anime7.status = Some("CANCELLED".into());
        db.upsert_catalog_anime(&anime7, 6).unwrap();

        let ids = db.stale_status_ids().unwrap();
        assert!(ids.contains(&1), "must include linked airing row");
        assert!(ids.contains(&2), "must include RELEASING row");
        assert!(ids.contains(&3), "must include NOT_YET_RELEASED row");
        assert!(ids.contains(&4), "must include FINISHED within 120 days");
        assert!(!ids.contains(&5), "must exclude old FINISHED row");
        assert!(!ids.contains(&6), "must exclude non-airing old FINISHED row");
        assert!(!ids.contains(&7), "must exclude CANCELLED row");
    }

    /// Regression: `stale_status_ids` correlates on `series.anilist_id` once per
    /// catalog row. Without an index on that column it scanned `series` for each
    /// of the ~22k catalog rows (twice), ~220 s on a real library, all while
    /// holding the DB mutex that the UI's synchronous commands need: the window
    /// froze. The plan must use the index, and a realistically sized library
    /// must answer in well under a second.
    #[test]
    fn stale_status_ids_uses_the_series_anilist_index_and_is_fast_at_real_size() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("TestSite", "https://site.example", "testsite").unwrap();

        // The plan: no full scan of `series` inside the correlated subqueries.
        let plan: Vec<String> = db
            .conn
            .prepare(&format!("EXPLAIN QUERY PLAN {STALE_STATUS_IDS_SQL}"))
            .unwrap()
            .query_map([0i64], |r| r.get::<_, String>(3))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert!(
            plan.iter().any(|d| d.contains("idx_series_anilist_id")),
            "correlated subquery must use idx_series_anilist_id, plan was: {plan:?}"
        );
        // A full scan of `series` (any SQLite wording: "SCAN s", "SCAN TABLE s", with or without
        // an alias suffix) would be the quadratic behaviour. Scans of `c` are expected.
        assert!(
            !plan.iter().any(|d| {
                let d = d.trim();
                d.starts_with("SCAN") && !d.contains(" c") && !d.contains("INDEX") && (d.ends_with(" s") || d.contains("TABLE s") || d.contains("series"))
            }),
            "series must not be fully scanned per catalog row, plan was: {plan:?}"
        );

        // Real-library size: 22k catalog rows, 6k series, ~100 airing and linked.
        let tx = db.conn.unchecked_transaction().unwrap();
        for i in 1..=22_000i64 {
            tx.execute(
                "INSERT INTO anilist_catalog (id, title, cover_url, format, episodes, average_score, url, sort_order, popularity, status)
                 VALUES (?1, ?2, NULL, 'TV', 12, 70, ?3, ?1, ?4, ?5)",
                (i, format!("Title {i}"), format!("https://anilist.co/anime/{i}"), i % 5000,
                 if i % 200 == 0 { "RELEASING" } else { "FINISHED" }),
            )
            .unwrap();
        }
        for i in 0..6_000i64 {
            let linked = if i % 3 == 0 { Some(i + 1) } else { None };
            tx.execute(
                "INSERT INTO series (source_id, slug, title, url, is_airing, anilist_id)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                (src, format!("s{i}"), format!("S {i}"), format!("https://x/{i}"), (i % 60 == 0) as i64, linked),
            )
            .unwrap();
        }
        tx.commit().unwrap();

        let started = std::time::Instant::now();
        let ids = db.stale_status_ids().unwrap();
        let took = started.elapsed();
        assert!(!ids.is_empty() && ids.len() <= 500);
        // Without the index this takes minutes; allow generous slack for slow CI debug builds.
        assert!(took < std::time::Duration::from_secs(5), "stale_status_ids took {took:?}");
    }

    #[test]
    fn stale_status_ids_orders_linked_first_and_respects_500_cap() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("TestSite", "https://site.example", "testsite").unwrap();

        // Create an unlinked RELEASING row with huge popularity
        let mut high_pop = catalog_anime_with_popularity(1001, "HighPopUnlinked", &[], Some(100_000));
        high_pop.status = Some("RELEASING".into());
        db.upsert_catalog_anime(&high_pop, 0).unwrap();

        // Create a linked row with low popularity
        let mut low_pop = catalog_anime_with_popularity(1002, "LowPopLinked", &[], Some(10));
        low_pop.status = Some("RELEASING".into());
        db.upsert_catalog_anime(&low_pop, 1).unwrap();
        let s = mk_airing("s1002", "LowPopLinked", None);
        let sid = db.upsert_series(src, &s).unwrap();
        db.set_anilist_id(sid, 1002).unwrap();

        let ids = db.stale_status_ids().unwrap();
        assert_eq!(ids[0], 1002, "linked row must come first despite lower popularity");
        assert_eq!(ids[1], 1001);

        // Populate > 500 rows and assert cap
        for i in 1..=550 {
            let mut a = catalog_anime_with_popularity(2000 + i, &format!("Batch{i}"), &[], Some(i));
            a.status = Some("RELEASING".into());
            db.upsert_catalog_anime(&a, i).unwrap();
        }
        let capped_ids = db.stale_status_ids().unwrap();
        assert_eq!(capped_ids.len(), 500, "must be capped at 500 rows");
    }

    #[test]
    fn upsert_catalog_anime_synonyms_and_catalog_titles_for_index() {
        let db = Db::open(":memory:").unwrap();
        let mut a = catalog_anime(1, "Main Title", &[]);
        a.synonyms = vec!["Synonym One".to_string(), "Synonym Two".to_string()];
        db.upsert_catalog_anime(&a, 0).unwrap();

        let titles = db.catalog_titles_for_index().unwrap();
        assert_eq!(titles.len(), 1);
        assert_eq!(titles[0].titles, vec!["Main Title".to_string()]);
        let mut actual = titles[0].synonyms.clone();
        actual.sort();
        assert_eq!(actual, vec!["Synonym One".to_string(), "Synonym Two".to_string()]);
    }

    #[test]
    fn upsert_catalog_anime_replaces_old_synonyms() {
        let db = Db::open(":memory:").unwrap();
        let mut a = catalog_anime(1, "Main Title", &[]);
        a.synonyms = vec!["Old Synonym".to_string()];
        db.upsert_catalog_anime(&a, 0).unwrap();

        let mut a_new = catalog_anime(1, "Main Title", &[]);
        a_new.synonyms = vec!["New Synonym".to_string()];
        db.upsert_catalog_anime(&a_new, 0).unwrap();

        let titles = db.catalog_titles_for_index().unwrap();
        assert_eq!(titles.len(), 1);
        assert_eq!(titles[0].synonyms, vec!["New Synonym".to_string()]);
    }

    #[test]
    fn catalog_titles_for_index_skips_short_synonyms_and_duplicates() {
        let db = Db::open(":memory:").unwrap();
        let mut a = catalog_anime(1, "Main Title", &[]);
        // "abc" is 3 chars (skip), "Main Title" is a duplicate of the main title (case-insensitive)
        a.synonyms = vec!["abc".to_string(), "main title".to_string(), "Valid Synonym".to_string()];
        db.upsert_catalog_anime(&a, 0).unwrap();

        let titles = db.catalog_titles_for_index().unwrap();
        assert_eq!(titles.len(), 1);
        assert_eq!(titles[0].synonyms, vec!["Valid Synonym".to_string()]);
    }

    #[test]
    fn link_series_to_catalog_matches_synonyms() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("TestSite", "https://site.example", "testsite").unwrap();

        let mut a = catalog_anime(4242, "Hyakunichi Kakumei", &[]);
        a.synonyms = vec!["Bai Ri Cheng Wang".to_string()];
        db.upsert_catalog_anime(&a, 0).unwrap();

        let sid = db.upsert_series(
            src,
            &crate::models::Series {
                id: 0,
                slug: "bai-ri-cheng-wang".into(),
                title: "Bai Ri Cheng Wang".into(),
                url: "https://site/bai-ri-cheng-wang".into(),
                cover_url: None,
                is_airing: true,
                followed: true,
                next_episode_at: None,
                site_episode_count: None,
            },
        ).unwrap();
        db.set_followed(sid, true).unwrap();

        let linked = db.link_series_to_catalog().unwrap();
        assert_eq!(linked, 1);

        let actual_id = db
            .conn
            .query_row(
                "SELECT anilist_id FROM series WHERE id=?1",
                [sid],
                |r| r.get::<_, Option<i64>>(0),
            )
            .unwrap();
        assert_eq!(actual_id, Some(4242));
    }

    #[test]
    fn upsert_catalog_anime_round_trips_next_airing_and_tags() {
        let db = Db::open(":memory:").unwrap();

        let mut a = catalog_anime(100, "Show", &[]);
        a.next_airing_at = Some(12345);
        a.next_episode = Some(5);
        a.tags = vec![
            crate::anilist::CatalogTag { name: "TagA".into(), rank: 90 },
            crate::anilist::CatalogTag { name: "TagB".into(), rank: 80 },
        ];
        db.upsert_catalog_anime(&a, 0).unwrap();

        let mut b = catalog_anime(101, "Finished", &[]);
        b.next_airing_at = None;
        b.next_episode = None;
        b.tags = vec![];
        db.upsert_catalog_anime(&b, 1).unwrap();

        let page = db.list_catalog(1, 10).unwrap();

        let read_a = page.iter().find(|x| x.id == 100).unwrap();
        assert_eq!(read_a.next_airing_at, Some(12345));
        assert_eq!(read_a.next_episode, Some(5));
        assert_eq!(read_a.tags.len(), 2);
        assert_eq!(read_a.tags[0].name, "TagA"); // sorted by rank DESC
        assert_eq!(read_a.tags[1].name, "TagB");

        let read_b = page.iter().find(|x| x.id == 101).unwrap();
        assert_eq!(read_b.next_airing_at, None);
        assert_eq!(read_b.next_episode, None);
        assert!(read_b.tags.is_empty());
    }

    #[test]
    fn upsert_catalog_anime_replaces_tags_and_clears_next_airing() {
        let db = Db::open(":memory:").unwrap();
        let mut a = catalog_anime(100, "Show", &[]);
        a.next_airing_at = Some(12345);
        a.next_episode = Some(5);
        a.tags = vec![crate::anilist::CatalogTag { name: "Old".into(), rank: 90 }];
        db.upsert_catalog_anime(&a, 0).unwrap();

        // Update: replace tags, clear next_airing_at to NULL (finished show)
        let mut a_new = catalog_anime(100, "Show", &[]);
        a_new.next_airing_at = None;
        a_new.next_episode = None;
        a_new.tags = vec![crate::anilist::CatalogTag { name: "New".into(), rank: 85 }];
        db.upsert_catalog_anime(&a_new, 0).unwrap();

        let page = db.list_catalog(1, 10).unwrap();
        let read = page.iter().find(|x| x.id == 100).unwrap();
        assert_eq!(read.next_airing_at, None);
        assert_eq!(read.next_episode, None);
        assert_eq!(read.tags.len(), 1);
        assert_eq!(read.tags[0].name, "New");
    }

    #[test]
    fn improved_search_rules_forget_the_old_misses_once() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("TestSite", "https://site.example", "testsite").unwrap();
        series_with(&db, src, "m", "Missed Title");
        let norm = crate::matching::normalize_title("Missed Title");
        let now = 20_000_000;

        db.set_setting("anilist_search_rules", "1").unwrap(); // misses recorded under old rules
        db.record_search_miss(&norm, now).unwrap();
        assert_eq!(db.unlinked_for_search(10, now).unwrap().len(), 1, "old-rules miss is forgotten");

        db.record_search_miss(&norm, now).unwrap();
        assert!(db.unlinked_for_search(10, now).unwrap().is_empty(), "a miss under the current rules stands");
    }

    #[test]
    fn unlinked_for_search_tests() {
        let db = Db::open(":memory:").unwrap();
        db.set_setting("anilist_search_rules", "2").unwrap(); // current rules: recorded misses stand
        let src = db.upsert_source("TestSite", "https://site.example", "testsite").unwrap();

        let now = 20_000_000;

        // s1: linked -> not offered
        let sid1 = db.upsert_series(src, &crate::models::Series {
            id: 0, slug: "s1".into(), title: "Linked Title".into(), url: "u1".into(), cover_url: None, is_airing: true, followed: true, next_episode_at: None, site_episode_count: None,
        }).unwrap();
        db.set_anilist_id(sid1, 123).unwrap();

        // s2: unlinked, no miss
        let sid2 = db.upsert_series(src, &crate::models::Series {
            id: 0, slug: "s2".into(), title: "Title 2".into(), url: "u2".into(), cover_url: None, is_airing: true, followed: true, next_episode_at: None, site_episode_count: None,
        }).unwrap();

        // s3: unlinked, missed 15 days ago -> offered
        let _sid3 = db.upsert_series(src, &crate::models::Series {
            id: 0, slug: "s3".into(), title: "Title 3".into(), url: "u3".into(), cover_url: None, is_airing: true, followed: true, next_episode_at: None, site_episode_count: None,
        }).unwrap();
        db.record_search_miss(&crate::matching::normalize_title("Title 3"), now - 15 * 24 * 3600).unwrap();

        // s4: unlinked, missed 13 days ago -> not offered
        let _sid4 = db.upsert_series(src, &crate::models::Series {
            id: 0, slug: "s4".into(), title: "Title 4".into(), url: "u4".into(), cover_url: None, is_airing: true, followed: true, next_episode_at: None, site_episode_count: None,
        }).unwrap();
        db.record_search_miss(&crate::matching::normalize_title("Title 4"), now - 13 * 24 * 3600).unwrap();

        let targets = db.unlinked_for_search(10, now).unwrap();
        assert_eq!(targets.len(), 2);

        // They should be "Title 2" and "Title 3"
        let t2 = targets.iter().find(|t| t.title == "Title 2");
        assert!(t2.is_some());
        let t3 = targets.iter().find(|t| t.title == "Title 3");
        assert!(t3.is_some());

        // Check link_search_result links all ids
        let a = catalog_anime(4242, "Linked Anime", &[]);
        db.link_search_result(&[sid2], &a).unwrap();

        // Check that sid2 is linked
        let actual_id: Option<i64> = db.conn.query_row("SELECT anilist_id FROM series WHERE id=?1", [sid2], |r| r.get(0)).unwrap();
        assert_eq!(actual_id, Some(4242));

        // Ensure catalog row is added
        let has_cat: i64 = db.conn.query_row("SELECT COUNT(*) FROM anilist_catalog WHERE id=4242", [], |r| r.get(0)).unwrap();
        assert_eq!(has_cat, 1);
    }

    #[test]
    fn link_resolves_a_temporada_title_through_the_bare_number_spelling() {
        // Real case: AniList lists season 2 as "... desu 2"; the site row says "... Temporada 2".
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "a", "animeytx").unwrap();
        let mut s2 = catalog_anime(159309, "Trapped in a Dating Sim Season 2", &[]);
        s2.synonyms = vec!["Otome Game Sekai wa Mob ni Kibishii Sekai desu 2".into()];
        db.upsert_catalog_anime(&s2, 0).unwrap();
        let row = series_with(&db, src, "otome2", "Otome Game Sekai wa Mob ni Kibishii Sekai desu Temporada 2");

        db.link_series_to_catalog().unwrap();
        assert_eq!(linked_id(&db, row), Some(159309));
    }

    #[test]
    fn a_temporada_title_never_takes_the_bare_number_of_an_unrelated_franchise() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "a", "animeytx").unwrap();
        // "Something Else 2" exists, but it is another show, not "Mob Show"'s season 2.
        db.upsert_catalog_anime(&catalog_anime(1, "Something Else 2", &[]), 0).unwrap();
        let row = series_with(&db, src, "mob2", "Mob Show Temporada 2");
        db.link_series_to_catalog().unwrap();
        assert_eq!(linked_id(&db, row), None);
    }

    #[test]
    fn query_catalog_matches_tag_isekai_and_genre_filter() {
        let db = Db::open(":memory:").unwrap();
        let mut a1 = catalog_anime(1001, "Tensei Shitara Slime Datta Ken", &["Fantasy"]);
        a1.tags = vec![crate::anilist::CatalogTag { name: "Isekai".into(), rank: 90 }];
        db.upsert_catalog_anime(&a1, 0).unwrap();

        let a2 = catalog_anime(1002, "Ordinary School Life", &["Action"]);
        db.upsert_catalog_anime(&a2, 0).unwrap();

        // Search text "isekai" matches a1 via tag even though title has "Slime" and genre is "Fantasy"
        let search_filter = CatalogFilter {
            search: Some("isekai".into()),
            ..Default::default()
        };
        let results = db.list_catalog_filtered(1, 10, &search_filter).unwrap();
        let total = db.catalog_count_filtered(&search_filter).unwrap();
        assert_eq!(total, 1);
        assert_eq!(results[0].id, 1001);

        // Genre filter with "Isekai" also matches a1 because genre filter checks tags as well
        let genre_filter = CatalogFilter {
            genres: vec!["Isekai".into()],
            ..Default::default()
        };
        let results2 = db.list_catalog_filtered(1, 10, &genre_filter).unwrap();
        let total2 = db.catalog_count_filtered(&genre_filter).unwrap();
        assert_eq!(total2, 1);
        assert_eq!(results2[0].id, 1001);

        // distinct_catalog_genres includes "Isekai"
        let genres = db.distinct_catalog_genres().unwrap();
        assert!(genres.contains(&"Isekai".to_string()));
    }

    #[test]
    fn test_catalog_filter_hide_watched_excludes_watched_externally_and_seen_episodes() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("AnimeYT", "https://animeyt.es", "animeytx").unwrap();

        let a1 = catalog_anime(1001, "Seen Externally Show", &[]);
        let a2 = catalog_anime(1002, "Watched In App Show", &[]);
        let a3 = catalog_anime(1003, "Unwatched Show", &[]);
        let a4 = catalog_anime(1004, "Unlinked Watched Show", &[]);

        db.upsert_catalog_anime(&a1, 0).unwrap();
        db.upsert_catalog_anime(&a2, 1).unwrap();
        db.upsert_catalog_anime(&a3, 2).unwrap();
        db.upsert_catalog_anime(&a4, 3).unwrap();

        // s1: linked to 1001, watched_externally = 1
        let s1_item = mk_airing("s1", "Seen Externally Show", None);
        let s1 = db.upsert_series(src, &s1_item).unwrap();
        db.set_anilist_id(s1, 1001).unwrap();
        db.set_watched_externally(s1, true).unwrap();

        // s2: linked to 1002, seen episode
        let s2_item = mk_airing("s2", "Watched In App Show", None);
        let s2 = db.upsert_series(src, &s2_item).unwrap();
        db.set_anilist_id(s2, 1002).unwrap();
        db.conn.execute(
            "INSERT INTO episodes (series_id, number, title, url, seen) VALUES (?1, '1', 'Ep 1', 'https://ep1', 1)",
            [s2],
        ).unwrap();

        // s3: linked to 1003, followed=1, 0 episodes seen, watched_externally=0
        let s3_item = mk_airing("s3", "Unwatched Show", None);
        let s3 = db.upsert_series(src, &s3_item).unwrap();
        db.set_anilist_id(s3, 1003).unwrap();

        // s4: unlinked (anilist_id is NULL), title matches a4, watched_externally=1
        let s4_item = mk_airing("s4", "Unlinked Watched Show", None);
        let s4 = db.upsert_series(src, &s4_item).unwrap();
        db.set_watched_externally(s4, true).unwrap();

        // When hide_watched is false: all 4 are returned
        let default_filter = CatalogFilter::default();
        let results = db.list_catalog_filtered(1, 10, &default_filter).unwrap();
        assert_eq!(results.len(), 4);

        // When hide_watched is true: only a3 (1003) is returned!
        let hide_watched_filter = CatalogFilter {
            hide_watched: true,
            ..Default::default()
        };
        let filtered = db.list_catalog_filtered(1, 10, &hide_watched_filter).unwrap();
        let total = db.catalog_count_filtered(&hide_watched_filter).unwrap();
        assert_eq!(total, 1);
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].id, 1003);
    }

    #[test]
    fn test_catalog_filter_max_score_excludes_higher_and_null() {
        let db = Db::open(":memory:").unwrap();
        let mut a1 = catalog_anime_with_popularity(1, "Low Score", &[], Some(100));
        a1.average_score = Some(45);
        let mut a2 = catalog_anime_with_popularity(2, "Boundary Score", &[], Some(100));
        a2.average_score = Some(59);
        let mut a3 = catalog_anime_with_popularity(3, "Just Over", &[], Some(100));
        a3.average_score = Some(60);
        let mut a4 = catalog_anime_with_popularity(4, "High Score", &[], Some(100));
        a4.average_score = Some(85);
        let mut a5 = catalog_anime_with_popularity(5, "Null Score", &[], Some(100));
        a5.average_score = None;

        db.upsert_catalog_anime(&a1, 0).unwrap();
        db.upsert_catalog_anime(&a2, 1).unwrap();
        db.upsert_catalog_anime(&a3, 2).unwrap();
        db.upsert_catalog_anime(&a4, 3).unwrap();
        db.upsert_catalog_anime(&a5, 4).unwrap();

        let filter = CatalogFilter {
            max_score: Some(59),
            ..Default::default()
        };
        let rows = db.list_catalog_filtered(1, 100, &filter).unwrap();
        let ids: Vec<i64> = rows.iter().map(|a| a.id).collect();
        assert_eq!(ids, vec![1, 2]);
        assert_eq!(db.catalog_count_filtered(&filter).unwrap(), 2);
    }

    #[test]
    fn test_catalog_filter_min_0_max_100_returns_all_scored_rows() {
        let db = Db::open(":memory:").unwrap();
        let mut a1 = catalog_anime_with_popularity(1, "Zero Score", &[], Some(100));
        a1.average_score = Some(0);
        let mut a2 = catalog_anime_with_popularity(2, "Mid Score", &[], Some(100));
        a2.average_score = Some(50);
        let mut a3 = catalog_anime_with_popularity(3, "Perfect Score", &[], Some(100));
        a3.average_score = Some(100);
        let mut a4 = catalog_anime_with_popularity(4, "Unscored", &[], Some(100));
        a4.average_score = None;

        db.upsert_catalog_anime(&a1, 0).unwrap();
        db.upsert_catalog_anime(&a2, 1).unwrap();
        db.upsert_catalog_anime(&a3, 2).unwrap();
        db.upsert_catalog_anime(&a4, 3).unwrap();

        let filter = CatalogFilter {
            min_score: Some(0),
            max_score: Some(100),
            ..Default::default()
        };
        let rows = db.list_catalog_filtered(1, 100, &filter).unwrap();
        let mut ids: Vec<i64> = rows.iter().map(|a| a.id).collect();
        ids.sort();
        assert_eq!(ids, vec![1, 2, 3]);
        assert_eq!(db.catalog_count_filtered(&filter).unwrap(), 3);
    }

    #[test]
    fn test_catalog_filter_start_date_inclusive_range_excludes_null() {
        let db = Db::open(":memory:").unwrap();
        let mut a1 = catalog_anime_with_popularity(1, "Too Early", &[], Some(100));
        a1.start_date = Some(1000);
        let mut a2 = catalog_anime_with_popularity(2, "Min Boundary", &[], Some(100));
        a2.start_date = Some(1500);
        let mut a3 = catalog_anime_with_popularity(3, "Max Boundary", &[], Some(100));
        a3.start_date = Some(2000);
        let mut a4 = catalog_anime_with_popularity(4, "Too Late", &[], Some(100));
        a4.start_date = Some(2500);
        let mut a5 = catalog_anime_with_popularity(5, "Undated", &[], Some(100));
        a5.start_date = None;

        db.upsert_catalog_anime(&a1, 0).unwrap();
        db.upsert_catalog_anime(&a2, 1).unwrap();
        db.upsert_catalog_anime(&a3, 2).unwrap();
        db.upsert_catalog_anime(&a4, 3).unwrap();
        db.upsert_catalog_anime(&a5, 4).unwrap();

        let filter = CatalogFilter {
            min_start_date: Some(1500),
            max_start_date: Some(2000),
            ..Default::default()
        };
        let rows = db.list_catalog_filtered(1, 100, &filter).unwrap();
        let mut ids: Vec<i64> = rows.iter().map(|a| a.id).collect();
        ids.sort();
        assert_eq!(ids, vec![2, 3]);
        assert_eq!(db.catalog_count_filtered(&filter).unwrap(), 2);
    }

    #[test]
    fn test_catalog_filter_status_finished_and_unknown_ignored() {
        let db = Db::open(":memory:").unwrap();
        let mut a1 = catalog_anime_with_popularity(1, "Finished Show", &[], Some(100));
        a1.status = Some("FINISHED".into());
        let mut a2 = catalog_anime_with_popularity(2, "Airing Show", &[], Some(100));
        a2.status = Some("RELEASING".into());
        let mut a3 = catalog_anime_with_popularity(3, "Upcoming Show", &[], Some(100));
        a3.status = Some("NOT_YET_RELEASED".into());
        let mut a4 = catalog_anime_with_popularity(4, "No Status Show", &[], Some(100));
        a4.status = None;

        db.upsert_catalog_anime(&a1, 0).unwrap();
        db.upsert_catalog_anime(&a2, 1).unwrap();
        db.upsert_catalog_anime(&a3, 2).unwrap();
        db.upsert_catalog_anime(&a4, 3).unwrap();

        // Exact match on FINISHED
        let finished_filter = CatalogFilter {
            status: Some("FINISHED".into()),
            ..Default::default()
        };
        let rows = db.list_catalog_filtered(1, 100, &finished_filter).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, 1);
        assert_eq!(db.catalog_count_filtered(&finished_filter).unwrap(), 1);

        // Unknown status value is ignored -> no filter applied
        let unknown_filter = CatalogFilter {
            status: Some("UNKNOWN_STATUS".into()),
            ..Default::default()
        };
        let all_rows = db.list_catalog_filtered(1, 100, &unknown_filter).unwrap();
        assert_eq!(all_rows.len(), 4);
        assert_eq!(db.catalog_count_filtered(&unknown_filter).unwrap(), 4);

        // SQL injection attempt in status: ignored
        let inj_filter = CatalogFilter {
            status: Some("' OR 1=1 --".into()),
            ..Default::default()
        };
        let inj_rows = db.list_catalog_filtered(1, 100, &inj_filter).unwrap();
        assert_eq!(inj_rows.len(), 4);
    }

    #[test]
    fn test_catalog_filter_sort_score_desc_and_oldest_with_id_ties() {
        let db = Db::open(":memory:").unwrap();
        let mut a1 = catalog_anime_with_popularity(10, "Show 10", &[], Some(100));
        a1.average_score = Some(80);
        a1.start_date = Some(2000);

        let mut a2 = catalog_anime_with_popularity(20, "Show 20", &[], Some(200));
        a2.average_score = Some(95);
        a2.start_date = Some(1995);

        let mut a3 = catalog_anime_with_popularity(5, "Show 5", &[], Some(50));
        a3.average_score = Some(80); // tie with a1, but id 5 < 10
        a3.start_date = Some(2010);

        let mut a4 = catalog_anime_with_popularity(30, "Show 30", &[], Some(300));
        a4.average_score = None; // null score
        a4.start_date = None;    // null date

        db.upsert_catalog_anime(&a1, 0).unwrap();
        db.upsert_catalog_anime(&a2, 1).unwrap();
        db.upsert_catalog_anime(&a3, 2).unwrap();
        db.upsert_catalog_anime(&a4, 3).unwrap();

        // sort=score_desc: 95 (id 20), then 80 (id 5 before id 10), then null (id 30)
        let score_desc_filter = CatalogFilter {
            sort: Some("score_desc".into()),
            ..Default::default()
        };
        let rows = db.list_catalog_filtered(1, 100, &score_desc_filter).unwrap();
        let ids: Vec<i64> = rows.iter().map(|a| a.id).collect();
        assert_eq!(ids, vec![20, 5, 10, 30]);

        // sort=oldest: 1995 (id 20), 2000 (id 10), 2010 (id 5), then null (id 30)
        let oldest_filter = CatalogFilter {
            sort: Some("oldest".into()),
            ..Default::default()
        };
        let rows_oldest = db.list_catalog_filtered(1, 100, &oldest_filter).unwrap();
        let ids_oldest: Vec<i64> = rows_oldest.iter().map(|a| a.id).collect();
        assert_eq!(ids_oldest, vec![20, 10, 5, 30]);

        // Unknown sort falls back to default popularity DESC NULLS LAST, id
        let unknown_sort_filter = CatalogFilter {
            sort: Some("random_garbage; DROP TABLE x;".into()),
            ..Default::default()
        };
        let rows_def = db.list_catalog_filtered(1, 100, &unknown_sort_filter).unwrap();
        let ids_def: Vec<i64> = rows_def.iter().map(|a| a.id).collect();
        assert_eq!(ids_def, vec![30, 20, 10, 5]);
    }

    #[test]
    fn test_catalog_filter_all_filters_combined_and_count_filtered_sync() {
        let db = Db::open(":memory:").unwrap();
        let mut a1 = catalog_anime_with_popularity(1, "Target Anime Movie", &["Action"], Some(500));
        a1.format = Some("MOVIE".into());
        a1.average_score = Some(75);
        a1.start_date = Some(1600000000);
        a1.status = Some("FINISHED".into());
        a1.studio = Some("Bones".into());
        a1.episodes = Some(1);

        let mut a2 = catalog_anime_with_popularity(2, "Other Target Anime", &["Action"], Some(400));
        a2.format = Some("MOVIE".into());
        a2.average_score = Some(80);
        a2.start_date = Some(1650000000);
        a2.status = Some("FINISHED".into());
        a2.studio = Some("Bones".into());
        a2.episodes = Some(1);

        let mut a3 = catalog_anime_with_popularity(3, "Fails Score Anime", &["Action"], Some(300));
        a3.format = Some("MOVIE".into());
        a3.average_score = Some(50); // score too low
        a3.start_date = Some(1600000000);
        a3.status = Some("FINISHED".into());
        a3.studio = Some("Bones".into());
        a3.episodes = Some(1);

        db.upsert_catalog_anime(&a1, 0).unwrap();
        db.upsert_catalog_anime(&a2, 1).unwrap();
        db.upsert_catalog_anime(&a3, 2).unwrap();

        let filter = CatalogFilter {
            search: Some("Target".into()),
            genres: vec!["Action".into()],
            format: Some("MOVIE".into()),
            min_score: Some(70),
            max_score: Some(85),
            min_start_date: Some(1500000000),
            max_start_date: Some(1700000000),
            status: Some("FINISHED".into()),
            studio: Some("Bones".into()),
            episodes: Some("1".into()),
            sort: Some("score_desc".into()),
            hide_watched: false,
        };

        let rows = db.list_catalog_filtered(1, 100, &filter).unwrap();
        let count = db.catalog_count_filtered(&filter).unwrap();
        assert_eq!(rows.len() as i64, count);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].id, 2); // 80 > 75
        assert_eq!(rows[1].id, 1);
    }

    #[test]
    fn test_deck_min_score_persistence_and_random_catalog_anime_filter() {
        let db = Db::open(":memory:").unwrap();
        assert_eq!(db.get_deck_min_score().unwrap(), None);
        db.set_deck_min_score(Some(75)).unwrap();
        assert_eq!(db.get_deck_min_score().unwrap(), Some(75));
        db.set_deck_min_score(None).unwrap();
        assert_eq!(db.get_deck_min_score().unwrap(), None);

        // Test random_catalog_anime_in_genre with min_score
        let mut low = catalog_anime_with_popularity(1, "LowScoreShow", &["Action"], Some(1000));
        low.average_score = Some(60);
        db.upsert_catalog_anime(&low, 0).unwrap();
        let mut high = catalog_anime_with_popularity(2, "HighScoreShow", &["Action"], Some(1000));
        high.average_score = Some(85);
        db.upsert_catalog_anime(&high, 1).unwrap();

        // Floor of 70 filters out LowScoreShow (60), picks HighScoreShow (85)
        for _ in 0..10 {
            let picked = db
                .random_catalog_anime_in_genre(
                    "Action",
                    &[],
                    &[],
                    &std::collections::HashSet::new(),
                    &std::collections::HashMap::new(),
                    &std::collections::HashMap::new(),
                    false,
                    false,
                    None,
                    None,
                    Some(70),
                )
                .unwrap()
                .unwrap();
            assert_eq!(picked.title, "HighScoreShow");
        }
    }


    #[test]
    fn test_replace_episode_map_replaces_previous_entries() {
        let db = Db::open(":memory:").unwrap();
        let entries_v1 = vec![
            crate::anizip::MapEntry { episode: 1, season: 1, absolute: Some(1), air_at: None },
            crate::anizip::MapEntry { episode: 2, season: 1, absolute: Some(2), air_at: None },
        ];
        db.replace_episode_map(100, &entries_v1, 1000).unwrap();
        assert_eq!(db.episode_offset(100).unwrap(), Some(0));

        let entries_v2 = vec![
            crate::anizip::MapEntry { episode: 1, season: 2, absolute: Some(25), air_at: None },
        ];
        db.replace_episode_map(100, &entries_v2, 2000).unwrap();
        assert_eq!(db.episode_offset(100).unwrap(), Some(24));

        let count: i64 = db.conn.query_row(
            "SELECT COUNT(*) FROM anilist_episode_map WHERE anilist_id = 100",
            [],
            |r| r.get(0),
        ).unwrap();
        assert_eq!(count, 1);

        let fetched: (i64, i64) = db.conn.query_row(
            "SELECT fetched_at, entries FROM anilist_episode_map_fetched WHERE anilist_id = 100",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        ).unwrap();
        assert_eq!(fetched, (2000, 1));
    }

    #[test]
    fn test_episode_offset_cases() {
        let db = Db::open(":memory:").unwrap();

        // 185756 starting at absolute 25 -> 25 - 1 = 24
        let entries_185756 = vec![
            crate::anizip::MapEntry { episode: 1, season: 3, absolute: Some(25), air_at: None },
            crate::anizip::MapEntry { episode: 2, season: 3, absolute: Some(26), air_at: None },
        ];
        db.replace_episode_map(185756, &entries_185756, 1000).unwrap();
        assert_eq!(db.episode_offset(185756).unwrap(), Some(24));

        // season-1 map (absolute 1) -> 1 - 1 = 0
        let entries_s1 = vec![
            crate::anizip::MapEntry { episode: 1, season: 1, absolute: Some(1), air_at: None },
        ];
        db.replace_episode_map(164702, &entries_s1, 1000).unwrap();
        assert_eq!(db.episode_offset(164702).unwrap(), Some(0));

        // None with no map
        assert_eq!(db.episode_offset(999999).unwrap(), None);

        // None when absolute is NULL
        let entries_no_abs = vec![
            crate::anizip::MapEntry { episode: 1, season: 1, absolute: None, air_at: None },
        ];
        db.replace_episode_map(888888, &entries_no_abs, 1000).unwrap();
        assert_eq!(db.episode_offset(888888).unwrap(), None);
    }

    #[test]
    fn test_episode_number_for_absolute_lookup() {
        let db = Db::open(":memory:").unwrap();
        let entries = vec![
            crate::anizip::MapEntry { episode: 1, season: 3, absolute: Some(25), air_at: None },
            crate::anizip::MapEntry { episode: 2, season: 3, absolute: Some(26), air_at: None },
            crate::anizip::MapEntry { episode: 3, season: 3, absolute: Some(27), air_at: None },
        ];
        db.replace_episode_map(185756, &entries, 1000).unwrap();
        assert_eq!(db.episode_number_for_absolute(185756, 27).unwrap(), Some(3));
        assert_eq!(db.episode_number_for_absolute(185756, 99).unwrap(), None);
        assert_eq!(db.episode_number_for_absolute(999999, 27).unwrap(), None);
    }

    #[test]
    fn test_episode_map_stale_ids_conditions() {
        let db = Db::open(":memory:").unwrap();
        let now = 1_000_000;
        let one_day = 86400;

        db.conn.execute("INSERT INTO sources (id, name, base_url, site_id) VALUES (1, 's', 'https://s.example', 'animeytx')", []).unwrap();

        let insert_series = |id: i64, anilist_id: Option<i64>, followed: i64, is_airing: i64| {
            db.conn.execute(
                "INSERT INTO series (id, source_id, slug, title, url, followed, is_airing, anilist_id)
                 VALUES (?1, 1, ?2, ?2, 'https://example.com', ?3, ?4, ?5)",
                rusqlite::params![id, format!("slug-{id}"), followed, is_airing, anilist_id],
            ).unwrap();
        };

        let insert_catalog = |id: i64, status: &str| {
            db.conn.execute(
                "INSERT INTO anilist_catalog (id, title, url, sort_order, status)
                 VALUES (?1, 'Title', 'https://anilist.co', 0, ?2)",
                rusqlite::params![id, status],
            ).unwrap();
        };

        let insert_fetched = |id: i64, fetched_at: i64| {
            db.conn.execute(
                "INSERT INTO anilist_episode_map_fetched (anilist_id, fetched_at, entries)
                 VALUES (?1, ?2, 1)",
                rusqlite::params![id, fetched_at],
            ).unwrap();
        };

        // 1. Followed linked series with no fetched row -> returned
        insert_series(1, Some(101), 1, 0);
        insert_catalog(101, "RELEASING");

        // 2. Fetched 1 day ago (RELEASING) -> NOT returned
        insert_series(2, Some(102), 1, 0);
        insert_catalog(102, "RELEASING");
        insert_fetched(102, now - one_day);

        // 3. Fetched 4 days ago (RELEASING) -> returned
        insert_series(3, Some(103), 1, 0);
        insert_catalog(103, "RELEASING");
        insert_fetched(103, now - 4 * one_day);

        // 4. Fetched 4 days ago (FINISHED) -> NOT returned
        insert_series(4, Some(104), 1, 0);
        insert_catalog(104, "FINISHED");
        insert_fetched(104, now - 4 * one_day);

        // 5. Fetched 31 days ago (FINISHED) -> returned
        insert_series(5, Some(105), 1, 0);
        insert_catalog(105, "FINISHED");
        insert_fetched(105, now - 31 * one_day);

        // 6. Unfollowed non-airing series -> NEVER returned
        insert_series(6, Some(106), 0, 0);
        insert_catalog(106, "RELEASING");

        // 7. Unlinked series (anilist_id NULL) -> NEVER returned
        insert_series(7, None, 1, 1);

        let stale = db.episode_map_stale_ids(now, 100).unwrap();
        assert!(stale.contains(&101), "no fetched row must be stale");
        assert!(!stale.contains(&102), "1 day ago RELEASING must not be stale");
        assert!(stale.contains(&103), "4 days ago RELEASING must be stale");
        assert!(!stale.contains(&104), "4 days ago FINISHED must not be stale");
        assert!(stale.contains(&105), "31 days ago FINISHED must be stale");
        assert!(!stale.contains(&106), "unfollowed non-airing must never be returned");
        assert_eq!(stale.len(), 3);

        // Limit honoured
        let limited = db.episode_map_stale_ids(now, 2).unwrap();
        assert_eq!(limited.len(), 2);
    }

    #[test]
    fn test_next_airing_episode_cases() {
        let db = Db::open(":memory:").unwrap();
        let now = 1_000_000;

        // 1. Map with episodes 1..3 at air_at now-100, now+100, now+200 -> Some((2, now+100))
        let entries = vec![
            crate::anizip::MapEntry { episode: 1, season: 1, absolute: Some(1), air_at: Some(now - 100) },
            crate::anizip::MapEntry { episode: 2, season: 1, absolute: Some(2), air_at: Some(now + 100) },
            crate::anizip::MapEntry { episode: 3, season: 1, absolute: Some(3), air_at: Some(now + 200) },
        ];
        db.replace_episode_map(100, &entries, now).unwrap();
        assert_eq!(db.next_airing_episode(100, now).unwrap(), Some((2, now + 100)));

        // 2. All entries in the past -> None
        let entries_past = vec![
            crate::anizip::MapEntry { episode: 1, season: 1, absolute: Some(1), air_at: Some(now - 200) },
            crate::anizip::MapEntry { episode: 2, season: 1, absolute: Some(2), air_at: Some(now - 100) },
            crate::anizip::MapEntry { episode: 3, season: 1, absolute: Some(3), air_at: Some(now) },
        ];
        db.replace_episode_map(200, &entries_past, now).unwrap();
        assert_eq!(db.next_airing_episode(200, now).unwrap(), None);

        // 3. Unknown anilist_id -> None
        assert_eq!(db.next_airing_episode(999999, now).unwrap(), None);

        // 4. Entry with NULL air_at ignored (others still found)
        let entries_with_null = vec![
            crate::anizip::MapEntry { episode: 1, season: 1, absolute: Some(1), air_at: None },
            crate::anizip::MapEntry { episode: 2, season: 1, absolute: Some(2), air_at: Some(now + 300) },
        ];
        db.replace_episode_map(300, &entries_with_null, now).unwrap();
        assert_eq!(db.next_airing_episode(300, now).unwrap(), Some((2, now + 300)));

        // 5. Two entries with the same future air_at -> lowest episode wins
        let entries_tie = vec![
            crate::anizip::MapEntry { episode: 5, season: 1, absolute: Some(5), air_at: Some(now + 500) },
            crate::anizip::MapEntry { episode: 4, season: 1, absolute: Some(4), air_at: Some(now + 500) },
        ];
        db.replace_episode_map(400, &entries_tie, now).unwrap();
        assert_eq!(db.next_airing_episode(400, now).unwrap(), Some((4, now + 500)));

        // 6. Past air_at must never be returned as upcoming
        let entries_strictly_past = vec![
            crate::anizip::MapEntry { episode: 1, season: 1, absolute: Some(1), air_at: Some(now - 1) },
        ];
        db.replace_episode_map(500, &entries_strictly_past, now).unwrap();
        assert_eq!(db.next_airing_episode(500, now).unwrap(), None);
    }

    #[test]
    fn test_import_catalog_snapshot_idempotent_and_no_overwrite() {
        let db = Db::open(":memory:").unwrap();
        // Seed an existing catalog row with id 1 and original title
        let existing = catalog_anime(1, "Original Title", &["Action"]);
        db.upsert_catalog_anime(&existing, 0).unwrap();

        // Snapshot items: id 1 with DIFFERENT title, and new id 2
        let item1 = catalog_anime(1, "Changed Title", &["Drama"]);
        let item2 = catalog_anime(2, "Brand New Anime", &["Comedy"]);

        let items = vec![item1, item2];

        // First import: id 1 skipped (not overwritten), id 2 inserted -> returns 1
        let inserted = db.import_catalog_snapshot(&items, 0).unwrap();
        assert_eq!(inserted, 1);
        assert_eq!(db.catalog_count().unwrap(), 2);

        // Check that id 1's title was NOT overwritten
        let (title, _, _) = db.get_catalog_titles(1).unwrap().unwrap();
        assert_eq!(title, "Original Title");

        // Second import with same items: returns 0 and nothing changes
        let second_inserted = db.import_catalog_snapshot(&items, 0).unwrap();
        assert_eq!(second_inserted, 0);
        assert_eq!(db.catalog_count().unwrap(), 2);
        let (title2, _, _) = db.get_catalog_titles(1).unwrap().unwrap();
        assert_eq!(title2, "Original Title");
    }

    #[test]
    fn test_import_catalog_snapshot_preserves_sort_order() {
        let db = Db::open(":memory:").unwrap();
        // Three anime items in a specific sequence
        let a10 = catalog_anime(10, "Tenth Anime", &["Action"]);
        let a20 = catalog_anime(20, "Twentieth Anime", &["Sci-Fi"]);
        let a5 = catalog_anime(5, "Fifth Anime", &["Fantasy"]);

        let items = vec![a10, a20, a5];
        let inserted = db.import_catalog_snapshot(&items, 0).unwrap();
        assert_eq!(inserted, 3);

        // catalog_all_for_snapshot orders by sort_order, id
        let read_back = db.catalog_all_for_snapshot().unwrap();
        assert_eq!(read_back.len(), 3);
        assert_eq!(read_back[0].id, 10);
        assert_eq!(read_back[1].id, 20);
        assert_eq!(read_back[2].id, 5);
        assert_eq!(read_back[0].title, "Tenth Anime");
        assert_eq!(read_back[1].title, "Twentieth Anime");
        assert_eq!(read_back[2].title, "Fifth Anime");
    }

    #[test]
    fn test_catalog_all_for_snapshot_full_fidelity_and_child_tables() {
        let db = Db::open(":memory:").unwrap();
        let full = crate::anilist::CatalogAnime {
            id: 42,
            title: "Main Title".into(),
            title_romaji: Some("Romaji Title".into()),
            title_english: Some("English Title".into()),
            synonyms: vec!["Synonym 1".into(), "Synonym 2".into()],
            cover_url: Some("https://example.com/cover.jpg".into()),
            banner_url: Some("https://example.com/banner.jpg".into()),
            format: Some("TV".into()),
            genres: vec!["Action".into(), "Adventure".into()],
            episodes: Some(24),
            average_score: Some(88),
            popularity: Some(12345),
            url: "https://anilist.co/anime/42".into(),
            status: Some("FINISHED".into()),
            duration: Some(25),
            studio: Some("Kyoto Animation".into()),
            start_date: Some(1609459200),
            next_airing_at: Some(1612137600),
            next_episode: Some(12),
            tags: vec![
                crate::anilist::CatalogTag { name: "Magic".into(), rank: 70 },
                crate::anilist::CatalogTag { name: "Isekai".into(), rank: 50 },
            ],
            relations: vec![
                crate::anilist::CatalogRelation { related_id: 100, relation_type: "PREQUEL".into() },
                crate::anilist::CatalogRelation { related_id: 101, relation_type: "SEQUEL".into() },
            ],
        };

        db.upsert_catalog_anime(&full, 0).unwrap();
        let snapshot = db.catalog_all_for_snapshot().unwrap();
        assert_eq!(snapshot.len(), 1);
        let item = &snapshot[0];
        assert_eq!(item.id, 42);
        assert_eq!(item.title, "Main Title");
        assert_eq!(item.title_romaji.as_deref(), Some("Romaji Title"));
        assert_eq!(item.title_english.as_deref(), Some("English Title"));
        assert_eq!(item.synonyms, vec!["Synonym 1", "Synonym 2"]);
        assert_eq!(item.cover_url.as_deref(), Some("https://example.com/cover.jpg"));
        assert_eq!(item.banner_url.as_deref(), Some("https://example.com/banner.jpg"));
        assert_eq!(item.format.as_deref(), Some("TV"));
        assert_eq!(item.genres, vec!["Action", "Adventure"]);
        assert_eq!(item.episodes, Some(24));
        assert_eq!(item.average_score, Some(88));
        assert_eq!(item.popularity, Some(12345));
        assert_eq!(item.url, "https://anilist.co/anime/42");
        assert_eq!(item.status.as_deref(), Some("FINISHED"));
        assert_eq!(item.duration, Some(25));
        assert_eq!(item.studio.as_deref(), Some("Kyoto Animation"));
        assert_eq!(item.start_date, Some(1609459200));
        assert_eq!(item.next_airing_at, Some(1612137600));
        assert_eq!(item.next_episode, Some(12));
        // Tags must be sorted by rank asc then name: 50 ("Isekai") before 70 ("Magic")
        assert_eq!(
            item.tags,
            vec![
                crate::anilist::CatalogTag { name: "Isekai".into(), rank: 50 },
                crate::anilist::CatalogTag { name: "Magic".into(), rank: 70 },
            ]
        );
        // Relations sorted by related_id asc then relation_type
        assert_eq!(
            item.relations,
            vec![
                crate::anilist::CatalogRelation { related_id: 100, relation_type: "PREQUEL".into() },
                crate::anilist::CatalogRelation { related_id: 101, relation_type: "SEQUEL".into() },
            ]
        );
    }

    #[test]
    fn build_catalog_airing_meta_matches_old_functions_map_by_map() {
        let db = Db::open(":memory:").unwrap();

        // Row 1: has title, romaji, english, start_date, banner, cover, score.
        let mut a1 = catalog_anime(1, "Shingeki no Kyojin", &[]);
        a1.title_romaji = Some("Attack on Titan Romaji".into());
        a1.title_english = Some("Attack on Titan".into());
        a1.start_date = Some(1365292800);
        a1.banner_url = Some("https://cdn/banner1.jpg".into());
        a1.cover_url = Some("https://cdn/cover1.jpg".into());
        a1.average_score = Some(85);
        db.upsert_catalog_anime(&a1, 0).unwrap();

        // Row 2: shares normalized variant "attack on titan" with a1.
        // First variant encountered wins, so a1's values must win for that key.
        let mut a2 = catalog_anime(2, "Attack on Titan", &[]);
        a2.title_romaji = None;
        a2.title_english = Some("AoT Alternative".into());
        a2.start_date = Some(1400000000);
        a2.banner_url = Some("https://cdn/banner2.jpg".into());
        a2.cover_url = Some("https://cdn/cover2.jpg".into());
        a2.average_score = Some(90);
        db.upsert_catalog_anime(&a2, 1).unwrap();

        // Row 3: empty banner_url and empty cover_url (""), NULL start_date, has score.
        let mut a3 = catalog_anime(3, "Empty Banner Show", &[]);
        a3.title_romaji = Some("Romaji Variant".into());
        a3.title_english = None;
        a3.start_date = None;
        a3.banner_url = Some("".into());
        a3.cover_url = Some("".into());
        a3.average_score = Some(70);
        db.upsert_catalog_anime(&a3, 2).unwrap();

        // Row 4: NULL score, NULL banner_url, has start_date and cover_url.
        let mut a4 = catalog_anime(4, "No Score Show", &[]);
        a4.title_romaji = None;
        a4.title_english = Some("No Score English".into());
        a4.start_date = Some(1500000000);
        a4.banner_url = None;
        a4.cover_url = Some("https://cdn/cover4.jpg".into());
        a4.average_score = None;
        db.upsert_catalog_anime(&a4, 3).unwrap();

        let old_dates = db.catalog_start_dates_by_normalized_title().unwrap();
        let old_banners = db.catalog_banners_by_normalized_title().unwrap();
        let old_covers = db.catalog_covers_by_normalized_title().unwrap();
        let old_scores = db.catalog_scores_by_normalized_title().unwrap();

        let rows = db.catalog_airing_meta_rows().unwrap();
        assert_eq!(rows.len(), 4);
        let meta = build_catalog_airing_meta(rows);

        assert_eq!(meta.start_dates, old_dates);
        assert_eq!(meta.banners, old_banners);
        assert_eq!(meta.covers, old_covers);
        assert_eq!(meta.scores, old_scores);

        let norm_shared = crate::matching::normalize_title("Attack on Titan");
        assert_eq!(meta.start_dates.get(&norm_shared), Some(&1365292800));
        assert_eq!(meta.banners.get(&norm_shared).map(String::as_str), Some("https://cdn/banner1.jpg"));
        assert_eq!(meta.covers.get(&norm_shared).map(String::as_str), Some("https://cdn/cover1.jpg"));
        assert_eq!(meta.scores.get(&norm_shared), Some(&85.0));

        let norm_empty = crate::matching::normalize_title("Empty Banner Show");
        assert!(!meta.banners.contains_key(&norm_empty));
        assert!(!meta.covers.contains_key(&norm_empty));
        assert!(!meta.start_dates.contains_key(&norm_empty));
        assert_eq!(meta.scores.get(&norm_empty), Some(&70.0));
    }

    #[test]
    fn series_banners_and_scores_matches_individual_queries() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("TestSite", "https://site.example", "testsite").unwrap();

        let mut c_a = catalog_anime(10, "Series A Catalog", &[]);
        c_a.banner_url = Some("https://cat/banner_a.jpg".into());
        c_a.average_score = Some(82);
        db.upsert_catalog_anime(&c_a, 0).unwrap();

        let sid_a = db.upsert_series(src, &mk_airing("series-a", "Series A", None)).unwrap();
        db.set_anilist_id(sid_a, 10).unwrap();
        db.conn.execute(
            "UPDATE series SET banner_url = ?1 WHERE id = ?2",
            rusqlite::params!["https://custom/banner_a.jpg", sid_a],
        ).unwrap();

        let mut c_b = catalog_anime(20, "Series B Catalog", &[]);
        c_b.banner_url = Some("https://cat/banner_b.jpg".into());
        c_b.average_score = Some(75);
        db.upsert_catalog_anime(&c_b, 1).unwrap();

        let sid_b = db.upsert_series(src, &mk_airing("series-b", "Series B", None)).unwrap();
        db.set_anilist_id(sid_b, 20).unwrap();

        let sid_c = db.upsert_series(src, &mk_airing("series-c", "Series C", None)).unwrap();

        let missing_id = 999_999i64;

        let series_ids = vec![sid_a, sid_b, sid_c, missing_id];
        let batch = db.series_banners_and_scores(&series_ids).unwrap();

        assert_eq!(
            batch.get(&sid_a),
            Some(&(db.get_series_banner(sid_a).unwrap(), db.get_series_score(sid_a).unwrap()))
        );
        assert_eq!(
            batch.get(&sid_a),
            Some(&(Some("https://custom/banner_a.jpg".to_string()), Some(82.0)))
        );

        assert_eq!(
            batch.get(&sid_b),
            Some(&(db.get_series_banner(sid_b).unwrap(), db.get_series_score(sid_b).unwrap()))
        );
        assert_eq!(
            batch.get(&sid_b),
            Some(&(Some("https://cat/banner_b.jpg".to_string()), Some(75.0)))
        );

        assert_eq!(
            batch.get(&sid_c),
            Some(&(db.get_series_banner(sid_c).unwrap(), db.get_series_score(sid_c).unwrap()))
        );
        assert_eq!(batch.get(&sid_c), Some(&(None, None)));

        assert_eq!(batch.get(&missing_id), None);
    }

    #[test]
    fn series_banners_and_scores_chunking_large_id_list() {
        let db = Db::open(":memory:").unwrap();
        let src = db.upsert_source("TestSite", "https://site.example", "testsite").unwrap();
        let sid = db.upsert_series(src, &mk_airing("s-one", "Series One", None)).unwrap();
        db.conn.execute(
            "UPDATE series SET banner_url = 'https://banner.jpg' WHERE id = ?1",
            [sid],
        ).unwrap();

        let mut ids = vec![sid];
        for i in 1..=600 {
            ids.push(1000 + i);
        }
        let batch = db.series_banners_and_scores(&ids).unwrap();
        assert_eq!(batch.len(), 1);
        assert_eq!(batch.get(&sid), Some(&(Some("https://banner.jpg".to_string()), None)));
    }
}
