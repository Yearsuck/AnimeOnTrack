use super::*;

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
