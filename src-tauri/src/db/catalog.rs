use super::*;
use crate::anilist::CatalogTag;
use rusqlite::types::Value;
use std::collections::HashMap;

/// (franchise key, season) -> linked series (id, title, anilist_id).
type SiblingIndex = std::collections::HashMap<(String, i64), Vec<(i64, String, String, i64)>>;

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
     ORDER BY (c.next_airing_at IS NOT NULL AND c.next_airing_at <= ?2) DESC,
              EXISTS (SELECT 1 FROM series s WHERE s.anilist_id = c.id) DESC,
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
    pub fn series_needing_catalog_link(&self) -> Result<Vec<(i64, String, String)>> {
        let mut stmt = self.conn.prepare(
            "SELECT s.id, s.title, s.slug FROM series s
             WHERE s.anilist_id IS NULL
               AND (s.followed=1 OR s.watched_externally=1 OR s.is_airing=1
                    OR EXISTS (SELECT 1 FROM episodes e WHERE e.series_id=s.id AND e.seen=1))
             ORDER BY s.id",
        )?;
        let rows = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
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
    fn sibling_index(linked: &[(i64, String, String, i64)]) -> SiblingIndex {
        let mut map: SiblingIndex = std::collections::HashMap::new();
        for (id, title, slug, anilist_id) in linked {
    
            // Ambiguous seasons ("The Final Season") must not pool with season 1.
            let Some(season) = crate::matching::effective_season(title, slug) else { continue };
            map.entry((crate::matching::franchise_dedup_key(title), season as i64))
                .or_default()
                .push((*id, title.clone(), slug.clone(), *anilist_id));
        }
        map
    }

    /// A show long-running enough that the sites split it into arc rows
    /// ("One Piece: Arco de Elbaph"). Only those may inherit their parent's
    /// link from the colon-stripped base title; for a short show the colon
    /// tail is a season ("Kami no Tou: Ouji no Kikan" is Tower of God season
    /// 2, not season 1). AniList leaves `episodes` empty while a show airs,
    /// so a NULL episode count alone does not make a show a long runner: we
    /// require `next_episode > 60` or >60 rows in `anilist_episode_map`.
    fn is_long_runner(&self, anilist_id: i64) -> Result<bool> {
        let row: Option<(Option<i64>, Option<i64>)> = self
            .conn
            .query_row(
                "SELECT episodes, next_episode FROM anilist_catalog WHERE id=?1",
                [anilist_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let Some((episodes, next_episode)) = row else {
            return Ok(false);
        };
        if let Some(n) = episodes {
            return Ok(n >= 60);
        }
        if next_episode.is_some_and(|ne| ne > 60) {
            return Ok(true);
        }
        let map_count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM anilist_episode_map WHERE anilist_id=?1",
            [anilist_id],
            |r| r.get(0),
        )?;
        Ok(map_count > 60)
    }

    /// The permissive counterpart of `is_long_runner`, for the repair pass that
    /// CLEARS existing links: there a missing total (or a missing catalog row)
    /// must keep the link. A long runner between airings or on hiatus has no
    /// countdown and may not have its episode map yet, and wiping a correct
    /// "One Piece: Arco de Elbaph" link on startup is worse than keeping a
    /// doubtful one. Creating new links uses the strict `is_long_runner`.
    fn may_be_long_runner(&self, anilist_id: i64) -> Result<bool> {
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
        slug: &str,
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
                crate::matching::season_consistent_with_slug(title, slug, &refs)
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


        let s = crate::matching::effective_season(title, slug);
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
                let bare = format!("{stem} {n}"); let s_cand = format!("{stem} Season {n}");
                for id in [index.lookup(&[&bare, &s_cand]), index.fuzzy_lookup(&[&bare, &s_cand])].into_iter().flatten() {
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
        for (sib_id, sib_title, sib_slug, sib_anilist_id) in siblings.get(&key).into_iter().flatten() {
            if *sib_id == ignore_series_id { continue; }
            if let Some(titles) = catalog_titles.get(sib_anilist_id) {
                let refs: Vec<&str> = titles.iter().map(String::as_str).collect();
                if crate::matching::season_consistent_with_slug(sib_title, sib_slug, &refs) {
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

        let mut stmt = self.conn.prepare("SELECT id, title, slug, anilist_id FROM series WHERE anilist_id IS NOT NULL")?;
        let rows = stmt.query_map([], |r| {
            let id: i64 = r.get(0)?;
            let title: String = r.get(1)?;
            let slug: String = r.get(2)?;
            let anilist_id: i64 = r.get(3)?;
            Ok((id, title, slug, anilist_id))
        })?;

        let mut linked_series = Vec::new();
        for row in rows {
            let (id, title, slug, anilist_id) = row?;
            linked_series.push((id, title.clone(), slug.clone(), anilist_id));

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
            // The row's own season uses the strict reading (explicit markers, trailing
            // numerals): a numeral before a colon ("Lupin III: ...") names a work, not a
            // season, and this pass removes links, so it must not guess.
            let s = crate::matching::effective_season(&title, &slug);
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
                let justified = self.may_be_long_runner(anilist_id)?
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
                    if crate::matching::season_consistent_with_slug(&title, &slug, &refs) {
                        return false;
                    }
                    let key = crate::matching::franchise_dedup_key(&title);
                    let base_key = crate::matching::franchise_dedup_key(
                        &crate::db::stats::franchise_base_title(&title),
                    );
                    let all_catalog_titles_plain = refs.iter().all(|t| {
                        crate::matching::extract_season_number_from_title(t).is_none_or(|n| n <= 1)
                    });
                    refs.iter().any(|t| {
                        let catalog_key = crate::matching::franchise_dedup_key(t);
                        catalog_key == key
                            || (all_catalog_titles_plain && catalog_key == base_key)
                            || crate::matching::extract_season_number_from_title(t).is_some_and(|n| n >= 2)
                    })
                });
                if contradicts {
                    to_repair.push((id, title, slug));
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
        for (id, title, slug) in to_repair {
            if let Some(new_id) = self.resolve_catalog_link(&title, &slug, id, index, catalog_titles, &siblings)? {
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

    #[cfg(test)] // superseded in production; kept as a test oracle
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
        let mut stmt = self.conn.prepare("SELECT id, title, slug, anilist_id FROM series WHERE anilist_id IS NOT NULL")?;
        let linked_series: Vec<(i64, String, String, i64)> = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?.collect::<rusqlite::Result<_>>()?;

        let siblings = Self::sibling_index(&linked_series);
        let mut linked = 0i64;
        for (series_id, title, slug) in self.series_needing_catalog_link()? {
            if let Some(new_id) = self.resolve_catalog_link(&title, &slug, series_id, index, &catalog_titles, &siblings)? {
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
    /// Core implementation of `random_catalog_anime_in_genre_with_signals`, also accepting
    /// `excluded_ids` so batches can exclude cards already picked in the same refill without
    /// waiting for them to be saved to `series`.
    #[allow(clippy::too_many_arguments)]
    pub fn random_catalog_anime_in_genre_with_signals_and_excluded_ids(
        &self,
        genre: &str,
        banned_formats: &[String],
        banned_genres: &[String],
        excluded_norm_titles: &std::collections::HashSet<String>,
        excluded_ids: &std::collections::HashSet<i64>,
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
        let mut sorted_excluded_ids: Vec<i64> = excluded_ids.iter().copied().collect();
        sorted_excluded_ids.sort_unstable();
        let excluded_id_clause = if sorted_excluded_ids.is_empty() {
            String::new()
        } else {
            let ph = sorted_excluded_ids.iter().map(|_| "?").collect::<Vec<_>>().join(", ");
            format!("AND c.id NOT IN ({ph})")
        };
        let sql = format!(
            "SELECT c.id, c.title, c.title_romaji, c.title_english, c.cover_url, c.banner_url, c.format, c.episodes, c.average_score, c.popularity, c.url, c.status, c.duration, c.studio, c.start_date, c.next_airing_at, c.next_episode
             FROM anilist_catalog c
             JOIN anilist_catalog_genres g ON g.anilist_id = c.id
             WHERE g.genre = ?
               AND c.format IN ({placeholders})
               AND c.popularity >= ?
               AND c.id NOT IN (SELECT anilist_id FROM series WHERE anilist_id IS NOT NULL)
               {excluded_id_clause}
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
        for id in &sorted_excluded_ids {
            params.push(Value::Integer(*id));
        }
        // Bound in the same order the clauses appear in the SQL above: genre,
        // formats, popularity, excluded ids, banned genres, date bounds, then score floor.
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
        let mut survivors: Vec<crate::anilist::CatalogAnime> = batch
            .into_iter()
            .filter(|a| !excluded_ids.contains(&a.id) && !is_engaged_by_title(a))
            .collect();
        let survivor_ids: Vec<i64> = survivors.iter().map(|a| a.id).collect();
        let mut genres_map = self.list_catalog_genres_for(&survivor_ids)?;
        let mut tags_map = self.list_catalog_tags_for(&survivor_ids)?;
        for anime in &mut survivors {
            anime.genres = genres_map.remove(&anime.id).unwrap_or_default();
            if anime.tags.is_empty() {
                anime.tags = tags_map.remove(&anime.id).unwrap_or_default();
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

    /// Same pick with nothing excluded by id. The app calls the `_and_excluded_ids` form
    /// (a deck batch excludes the cards it already picked); tests keep this shape.
    #[cfg(test)]
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
        self.random_catalog_anime_in_genre_with_signals_and_excluded_ids(
            genre,
            banned_formats,
            banned_genres,
            excluded_norm_titles,
            &std::collections::HashSet::new(),
            genre_affinity,
            format_affinity,
            taste_signals,
            recommended,
            hide_upcoming,
            min_start_date,
            max_start_date,
            min_score,
        )
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

#[cfg(test)]
mod tests;
